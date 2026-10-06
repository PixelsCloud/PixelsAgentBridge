use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitQuery {
    #[serde(default, skip_serializing_if = "crate::ExecutionSelection::is_service")]
    pub execution: crate::ExecutionSelection,
    pub repo: String,
    pub action: GitAction,
    pub timeout_ms: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitAction {
    Status {
        limit: u16,
    },
    Diff {
        staged: bool,
        base: Option<String>,
        head: Option<String>,
        paths: Vec<String>,
        context_lines: u8,
        max_bytes: u32,
    },
    Log {
        start: Option<String>,
        skip: u32,
        limit: u16,
    },
    Commit {
        files: Vec<String>,
        message: String,
    },
    Checkout {
        reference: String,
        detach: bool,
    },
    Fetch {
        remote: String,
        branch: Option<String>,
    },
    Pull {
        remote: String,
        branch: String,
        strategy: GitPullStrategy,
    },
    Push {
        remote: String,
        branch: String,
        force: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitPullStrategy {
    FfOnly,
    Merge,
    Rebase,
}
impl GitQuery {
    pub fn kind(&self) -> &'static str {
        match self.action {
            GitAction::Status { .. } => "git_status",
            GitAction::Diff { .. } => "git_diff",
            GitAction::Log { .. } => "git_log",
            GitAction::Commit { .. } => "git_commit",
            GitAction::Checkout { .. } => "git_checkout",
            GitAction::Fetch { .. } => "git_fetch",
            GitAction::Pull { .. } => "git_pull",
            GitAction::Push { .. } => "git_push",
        }
    }
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self.action,
            GitAction::Status { .. } | GitAction::Diff { .. } | GitAction::Log { .. }
        )
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if matches!(
            self.execution,
            crate::ExecutionSelection::DesktopUser { .. }
        ) {
            return Err("Git supports service or user execution, not desktop_user");
        }
        if !serde_json::to_vec(self).is_ok_and(|b| b.len() <= 60 * 1024) {
            return Err(
                "Git request exceeds the 60 KiB control payload budget; select fewer files",
            );
        }
        if self.repo.is_empty()
            || self.repo.len() > 4096
            || self.repo.contains('\0')
            || !(100..=300_000).contains(&self.timeout_ms)
        {
            return Err("repo must be a nonempty path (up to 4096 bytes); timeout_ms 100..300000");
        }
        let reference = |s: &str| {
            !s.is_empty()
                && s.len() <= 256
                && !s.starts_with('-')
                && !s.chars().any(char::is_control)
        };
        let remote = |s: &str| {
            reference(s)
                && s.len() <= 128
                && s.as_bytes()
                    .first()
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        };
        let paths = |p: &[String]| {
            p.len() <= 128
                && p.iter().all(|s| {
                    !s.is_empty()
                        && s.len() <= 4096
                        && !s.contains('\0')
                        && !s.starts_with(['/', '\\'])
                        && !s.contains(':')
                        && s.split(['/', '\\'])
                            .all(|c| c != ".." && !c.is_empty() && c != ".")
                })
        };
        match &self.action {
            GitAction::Status { limit } | GitAction::Log { limit, .. }
                if !(1..=1000).contains(limit) =>
            {
                return Err("limit 1..1000");
            }
            GitAction::Diff {
                staged,
                base,
                head,
                paths: p,
                context_lines,
                max_bytes,
            } if *context_lines > 20
                || !(1024..=16384).contains(max_bytes)
                || !paths(p)
                || base.as_ref().is_some_and(|s| !reference(s))
                || head.as_ref().is_some_and(|s| !reference(s))
                || (head.is_some() && base.is_none())
                || (*staged && base.is_some()) =>
            {
                return Err(
                    "invalid diff selection; staged and references are exclusive, head requires base, max_bytes 1024..16384, context_lines 0..20",
                );
            }
            GitAction::Log { start, skip, .. }
                if *skip > 100_000 || start.as_ref().is_some_and(|s| !reference(s)) =>
            {
                return Err("invalid log start/skip");
            }
            GitAction::Commit { files, message }
                if files.is_empty()
                    || !paths(files)
                    || message.trim().is_empty()
                    || message.len() > 8192
                    || message.contains('\0') =>
            {
                return Err(
                    "commit requires 1..128 literal relative files and a nonempty message up to 8192 bytes",
                );
            }
            GitAction::Checkout { reference: r, .. } if !reference(r) => {
                return Err("invalid checkout reference");
            }
            GitAction::Fetch { remote: r, branch }
                if !remote(r) || branch.as_ref().is_some_and(|s| !reference(s)) =>
            {
                return Err("invalid remote name/branch");
            }
            GitAction::Pull {
                remote: r, branch, ..
            }
            | GitAction::Push {
                remote: r, branch, ..
            } if !remote(r) || !reference(branch) => return Err("invalid remote name/branch"),
            _ => {}
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub repo: String,
    pub git_dir: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub unborn: bool,
    pub detached: bool,
    pub phase: String,
    pub index_may_have_changed: bool,
    pub head_after: Option<String>,
    pub start_commit: Option<String>,
    pub next_skip: Option<u32>,
    pub log_skip: Option<u32>,
    pub remote_reference: Option<String>,
    pub remote_oid_before: Option<String>,
    pub remote_oid_after: Option<String>,
    pub remote_observed_at_unix_ms: Option<i64>,
    pub entries: Vec<GitStatusEntry>,
    pub commits: Vec<GitCommitInfo>,
    pub diff: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub command_completed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatusEntry {
    pub index: String,
    pub worktree: String,
    pub path: String,
    pub path_hex: Option<String>,
    pub previous_path: Option<String>,
    pub previous_path_hex: Option<String>,
    pub conflict: bool,
    pub untracked: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitInfo {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub timestamp_seconds: i64,
    pub subject: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::SystemQuery;
    #[test]
    fn git_contract_rejects_injection_paths_and_conflicting_diff_and_hashes_commit_identity() {
        let mut q = GitQuery {
            execution: Default::default(),
            repo: "C:\\repo".into(),
            action: GitAction::Commit {
                files: vec!["中文 空格.txt".into()],
                message: "private commit message".into(),
            },
            timeout_ms: 30000,
        };
        q.validate().unwrap();
        let system = SystemQuery::Git { query: q.clone() };
        assert_eq!(system.required_version(), 5);
        assert!(system.is_mutation());
        assert!(
            !serde_json::to_string(&system.persistence_form())
                .unwrap()
                .contains("private commit message")
        );
        for path in [
            "../outside",
            "C:\\outside",
            "/outside",
            ":(glob)*",
            "x/../../outside",
            "bad\0file",
        ] {
            q.action = GitAction::Commit {
                files: vec![path.into()],
                message: "message".into(),
            };
            assert!(q.validate().is_err(), "{path}");
        }
        q.action = GitAction::Commit {
            files: vec!["long".repeat(256); 128],
            message: "too many bytes for a control frame".into(),
        };
        assert!(q.validate().unwrap_err().contains("60 KiB"));
        q.action = GitAction::Push {
            remote: "https://user:password@example.com".into(),
            branch: "main".into(),
            force: false,
        };
        assert!(q.validate().is_err());
        q.action = GitAction::Checkout {
            reference: "--force".into(),
            detach: false,
        };
        assert!(q.validate().is_err());
        q.action = GitAction::Diff {
            staged: true,
            base: Some("HEAD".into()),
            head: None,
            paths: vec![],
            context_lines: 3,
            max_bytes: 16384,
        };
        assert!(q.validate().is_err());
    }
}
