//! Native Git owns repository mechanics, hooks, locking, credentials and transport.
//! Product code supplies argv, bounded pipe readers, identity, and durable result facts.
use crate::task_store::TaskStore;
use pab_protocol::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{Mutex, watch},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
static REPOS: OnceLock<Mutex<HashMap<PathBuf, std::sync::Weak<Mutex<()>>>>> = OnceLock::new();
struct Failure {
    message: String,
    unconfirmed: bool,
    cancelled: bool,
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            unconfirmed: false,
            cancelled: false,
        }
    }
}
struct Output {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    overflow: bool,
}
struct Runner {
    program: PathBuf,
    cwd: PathBuf,
    deadline: Instant,
    cancelled: watch::Receiver<bool>,
    r: SystemQueryReply,
    snapshot: GitSnapshot,
    sink: GitSink,
    effects_started: bool,
}

#[derive(Clone)]
pub(crate) enum GitSink {
    Store(Option<TaskStore>),
    Worker(tokio::sync::mpsc::Sender<GitEvent>),
}
pub(crate) enum GitEvent {
    Lock(PathBuf, tokio::sync::oneshot::Sender<Result<(), String>>),
    Progress(
        SystemQueryReply,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .into()
}
fn cut(s: &mut String, n: usize) {
    let mut end = n.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}
async fn pipe(mut read: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut overflow = false;
    let mut buf = [0u8; 8192];
    loop {
        let n = read.read(&mut buf).await?;
        if n == 0 {
            return Ok((kept, overflow));
        }
        let take = n.min(OUTPUT_LIMIT.saturating_sub(kept.len()));
        kept.extend_from_slice(&buf[..take]);
        overflow |= take != n;
    }
}
pub(crate) async fn repo_lock(path: PathBuf) -> Arc<Mutex<()>> {
    let mut locks = REPOS.get_or_init(Mutex::default).lock().await;
    locks.retain(|_, w| w.strong_count() > 0);
    if let Some(lock) = locks.get(&path).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path, Arc::downgrade(&lock));
    lock
}
impl Runner {
    async fn worker_ack(
        &mut self,
        receive: tokio::sync::oneshot::Receiver<Result<(), String>>,
    ) -> Result<(), Failure> {
        let outcome = tokio::select! {
            r=receive=>r.unwrap_or_else(|_|Err("user worker coordinator disconnected".into())),
            _=tokio::time::sleep_until(self.deadline)=>Err("Git coordinator deadline exceeded".into()),
            _=self.cancelled.wait_for(|v|*v)=>Err("Git cancellation requested".into()),
        };
        outcome.map_err(|message| Failure {
            message,
            unconfirmed: self.effects_started,
            cancelled: *self.cancelled.borrow(),
        })
    }
    async fn lock_repository(
        &mut self,
        path: PathBuf,
    ) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, Failure> {
        if let GitSink::Worker(send) = &self.sink {
            let (answer, receive) = tokio::sync::oneshot::channel();
            send.send(GitEvent::Lock(path, answer))
                .await
                .map_err(|_| Failure::from("Git coordinator unavailable".to_owned()))?;
            self.worker_ack(receive).await?;
            Ok(None)
        } else {
            let lock = repo_lock(path).await;
            lock.try_lock_owned().map(Some).map_err(|_| {
                Failure::from("git_repository_busy: another PAB Git operation is active".to_owned())
            })
        }
    }
    async fn run(&mut self, args: &[String], effects: bool) -> Result<Output, Failure> {
        if *self.cancelled.borrow() {
            return Err(Failure {
                message: "Git cancellation requested".into(),
                unconfirmed: self.effects_started,
                cancelled: true,
            });
        }
        if Instant::now() >= self.deadline {
            return Err(Failure {
                message: "Git operation deadline exceeded".into(),
                unconfirmed: self.effects_started,
                cancelled: false,
            });
        }
        let mut command = Command::new(&self.program);
        command
            .current_dir(&self.cwd)
            .args([
                "--no-pager",
                "--no-optional-locks",
                "-c",
                "color.ui=false",
                "-c",
                "core.quotepath=false",
            ])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true");
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_CEILING_DIRECTORIES",
        ] {
            command.env_remove(key);
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|e| {
            Failure::from(if e.kind() == std::io::ErrorKind::NotFound {
                "git_not_installed: install Git on the target device".into()
            } else {
                format!("cannot start Git: {e}")
            })
        })?;
        self.effects_started |= effects;
        let mut out = tokio::spawn(pipe(child.stdout.take().expect("stdout piped")));
        let mut err = tokio::spawn(pipe(child.stderr.take().expect("stderr piped")));
        let wait = tokio::select! {
            result = child.wait() => result.map_err(|e| Failure::from(e.to_string())),
            _ = tokio::time::sleep_until(self.deadline) => Err(Failure { message:"Git operation deadline exceeded; accepted effects are not rolled back".into(), unconfirmed:self.effects_started, cancelled:false }),
            _ = self.cancelled.changed() => Err(Failure { message:"Git cancellation requested; accepted effects are not rolled back".into(), unconfirmed:self.effects_started, cancelled:true }),
        };
        let status = match wait {
            Ok(status) => status,
            Err(e) => {
                let _ = child.kill().await;
                out.abort();
                err.abort();
                return Err(e);
            }
        };
        // Hooks/SSH helpers may inherit pipes. Do not wait indefinitely for descendant handles.
        let pipes = tokio::select! { result=tokio::time::timeout_at(self.deadline, async {
            Ok::<_, String>((
                (&mut out)
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?,
                (&mut err)
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?,
            ))
        }) => result,
        _ = self.cancelled.changed() => {
            out.abort();err.abort();return Err(Failure {message:"Git cancellation requested while collecting output; effects are not rolled back".into(),unconfirmed:self.effects_started,cancelled:true});
        }};
        let ((stdout, a), (stderr, b)) = match pipes {
            Ok(Ok(data)) => data,
            _ => {
                out.abort();
                err.abort();
                return Err(Failure {
                    message: "Git output pipes did not close before deadline".into(),
                    unconfirmed: self.effects_started,
                    cancelled: false,
                });
            }
        };
        Ok(Output {
            code: status.code(),
            stdout,
            stderr,
            overflow: a || b,
        })
    }
    async fn checked(&mut self, args: Vec<String>, effects: bool) -> Result<Output, Failure> {
        if effects {
            self.snapshot.command_completed = false;
            self.snapshot.exit_code = None;
        }
        let output = self.run(&args, effects).await?;
        if effects {
            self.snapshot.exit_code = output.code;
            self.snapshot.command_completed = true;
            let mut stdout = text(&output.stdout);
            cut(&mut stdout, 2048);
            let mut stderr = text(&output.stderr);
            cut(&mut stderr, 2048);
            self.snapshot.stdout = Some(stdout);
            self.snapshot.stderr = Some(stderr);
        }
        if output.overflow {
            return Err(Failure {
                message: "git_output_limit: output exceeded 4 MiB scan budget; narrow the query"
                    .into(),
                unconfirmed: effects,
                cancelled: false,
            });
        }
        if output.code != Some(0) {
            let mut error = text(&output.stderr);
            cut(&mut error, 2048);
            return Err(Failure {
                message: format!("Git exited {:?}: {error}", output.code),
                unconfirmed: self.effects_started && output.code.is_none(),
                cancelled: false,
            });
        }
        Ok(output)
    }
    async fn stage(&mut self, phase: &str) -> Result<(), Failure> {
        self.snapshot.phase = phase.into();
        self.snapshot.command_completed = false;
        self.snapshot.exit_code = None;
        let r = bounded(self.r.clone(), self.snapshot.clone());
        match &self.sink {
            GitSink::Store(Some(store)) => {
                store.save_system_progress(&r).await.map_err(|e| Failure {
                    message: format!("cannot persist Git phase: {e}"),
                    unconfirmed: self.effects_started,
                    cancelled: false,
                })?
            }
            GitSink::Store(None) => {}
            GitSink::Worker(send) => {
                let (answer, receive) = tokio::sync::oneshot::channel();
                send.send(GitEvent::Progress(r, answer))
                    .await
                    .map_err(|_| Failure {
                        message: "Git coordinator disconnected".into(),
                        unconfirmed: self.effects_started,
                        cancelled: false,
                    })?;
                self.worker_ack(receive).await?;
            }
        }
        Ok(())
    }
    async fn resolve(&mut self, reference: &str) -> Result<String, Failure> {
        let out = self
            .checked(
                vec![
                    "rev-parse".into(),
                    "--verify".into(),
                    "--end-of-options".into(),
                    format!("{reference}^{{commit}}"),
                ],
                false,
            )
            .await?;
        let oid = text(&out.stdout);
        if !valid_oid(&oid) {
            return Err("Git returned invalid commit ID".to_owned().into());
        }
        Ok(oid)
    }
    async fn branch_name(&mut self, branch: &str) -> Result<(), Failure> {
        self.checked(
            vec!["check-ref-format".into(), format!("refs/heads/{branch}")],
            false,
        )
        .await?;
        Ok(())
    }
    async fn selected_files(&mut self, files: &[String]) -> Result<(), Failure> {
        for file in files {
            match tokio::fs::symlink_metadata(self.cwd.join(file)).await {
                Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink()=>{},
                Ok(_)=>return Err("commit files must name individual regular files/symlinks, not directories or special files".to_owned().into()),
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{
                    let out=self.checked(vec!["ls-files".into(),"-z".into(),"--error-unmatch".into(),"--".into(),file.clone()],false).await?;
                    let expected=if cfg!(windows){file.replace('\\',"/")}else{file.clone()};
                    if out.stdout!=format!("{expected}\0").as_bytes(){return Err("deleted file selection is not one exact tracked path".to_owned().into());}
                },
                Err(e)=>return Err(format!("cannot inspect selected file: {e}").into()),
            }
        }
        Ok(())
    }
    async fn perform(&mut self, q: &GitQuery) -> Result<(), Failure> {
        self.stage("resolving_repository").await?;
        let dir = self
            .checked(vec!["rev-parse".into(), "--absolute-git-dir".into()], false)
            .await?;
        let git_dir = tokio::fs::canonicalize(text(&dir.stdout))
            .await
            .map_err(|e| Failure::from(format!("repository unavailable: {e}")))?;
        self.snapshot.git_dir = git_dir.to_string_lossy().into_owned();
        let bare = self
            .checked(
                vec!["rev-parse".into(), "--is-bare-repository".into()],
                false,
            )
            .await?;
        if text(&bare.stdout) != "true" {
            let root = self
                .checked(vec!["rev-parse".into(), "--show-toplevel".into()], false)
                .await?;
            self.cwd = tokio::fs::canonicalize(text(&root.stdout))
                .await
                .map_err(|e| Failure::from(e.to_string()))?;
        } else if !matches!(
            q.action,
            GitAction::Log { .. } | GitAction::Fetch { .. } | GitAction::Push { .. }
        ) {
            return Err("operation requires a working tree; repository is bare"
                .to_owned()
                .into());
        }
        self.snapshot.repo = self.cwd.to_string_lossy().into_owned();
        let _guard = self.lock_repository(git_dir).await?;
        let head = self
            .run(
                &["rev-parse".into(), "--verify".into(), "HEAD".into()],
                false,
            )
            .await?;
        self.snapshot.head = (head.code == Some(0)).then(|| text(&head.stdout));
        let branch = self
            .run(
                &[
                    "symbolic-ref".into(),
                    "-q".into(),
                    "--short".into(),
                    "HEAD".into(),
                ],
                false,
            )
            .await?;
        self.snapshot.branch = (branch.code == Some(0)).then(|| text(&branch.stdout));
        if self.snapshot.head.is_none() && self.snapshot.branch.is_none() {
            return Err("repository HEAD is unavailable".to_owned().into());
        }
        self.snapshot.unborn = self.snapshot.head.is_none();
        self.snapshot.detached = self.snapshot.branch.is_none();
        match &q.action {
            GitAction::Status { limit } => {
                self.stage("reading_status").await?;
                let out = self
                    .checked(
                        vec![
                            "status".into(),
                            "--porcelain=v1".into(),
                            "-z".into(),
                            "--untracked-files=normal".into(),
                        ],
                        false,
                    )
                    .await?;
                let entries = parse_status(&out.stdout).map_err(Failure::from)?;
                self.r.truncated = entries.len() > usize::from(*limit);
                self.snapshot.entries = entries.into_iter().take(usize::from(*limit)).collect();
                if self.r.truncated {
                    self.r.stop_reason = Some("entry_limit".into());
                }
                if !self.snapshot.unborn && !self.snapshot.detached {
                    let upstream = self
                        .run(
                            &[
                                "rev-parse".into(),
                                "--symbolic-full-name".into(),
                                "@{upstream}".into(),
                            ],
                            false,
                        )
                        .await?;
                    if upstream.code == Some(0) {
                        let upstream = text(&upstream.stdout);
                        let counts = self
                            .checked(
                                vec![
                                    "rev-list".into(),
                                    "--left-right".into(),
                                    "--count".into(),
                                    format!("HEAD...{upstream}"),
                                    "--".into(),
                                ],
                                false,
                            )
                            .await?;
                        let n = text(&counts.stdout);
                        let mut n = n.split_whitespace();
                        self.snapshot.ahead = n.next().and_then(|s| s.parse().ok());
                        self.snapshot.behind = n.next().and_then(|s| s.parse().ok());
                        self.snapshot.upstream = Some(upstream);
                    }
                }
            }
            GitAction::Diff {
                staged,
                base,
                head,
                paths,
                context_lines,
                max_bytes,
            } => {
                self.stage("reading_diff").await?;
                let mut args = vec![
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--no-color".into(),
                    format!("--unified={context_lines}"),
                ];
                if *staged {
                    args.push("--cached".into());
                }
                if let Some(base) = base {
                    args.push(self.resolve(base).await?);
                }
                if let Some(head) = head {
                    args.push(self.resolve(head).await?);
                }
                args.push("--".into());
                args.extend(paths.iter().cloned());
                let out = self.checked(args, false).await?;
                let lossy = std::str::from_utf8(&out.stdout).is_err();
                let mut diff = String::from_utf8_lossy(&out.stdout).into_owned();
                if diff.len() > *max_bytes as usize {
                    cut(&mut diff, *max_bytes as usize);
                    self.r.truncated = true;
                    self.r.stop_reason = Some("diff_byte_limit".into());
                }
                if lossy {
                    self.r.warnings.push("diff contains non-UTF-8 bytes displayed with replacement characters; not an exact patch".into());
                }
                self.snapshot.diff = Some(diff);
            }
            GitAction::Log { start, skip, limit } => {
                self.snapshot.log_skip = Some(*skip);
                self.stage("reading_history").await?;
                if self.snapshot.unborn && start.is_none() {
                    self.r
                        .warnings
                        .push("unborn repository has no commits".into());
                } else {
                    let start = self.resolve(start.as_deref().unwrap_or("HEAD")).await?;
                    self.snapshot.start_commit = Some(start.clone());
                    let out = self
                        .checked(
                            vec![
                                "log".into(),
                                "--topo-order".into(),
                                format!("--skip={skip}"),
                                format!("--max-count={}", u32::from(*limit) + 1),
                                "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%s%x00".into(),
                                start,
                                "--".into(),
                            ],
                            false,
                        )
                        .await?;
                    self.snapshot.commits = parse_log(&out.stdout).map_err(Failure::from)?;
                    if self.snapshot.commits.len() > usize::from(*limit) {
                        self.snapshot.commits.truncate(usize::from(*limit));
                        self.snapshot.next_skip = Some(*skip + u32::from(*limit));
                        self.r.truncated = true;
                        self.r.stop_reason = Some("log_page_limit".into());
                    }
                }
            }
            GitAction::Commit { files, message } => {
                self.stage("checking_selected_files").await?;
                self.selected_files(files).await?;
                self.stage("staging_selected_files").await?;
                let mut add = vec!["add".into(), "--".into()];
                add.extend(files.iter().cloned());
                self.snapshot.index_may_have_changed = true;
                self.checked(add, true).await?;
                self.stage("committing_selected_files").await?;
                let mut commit = vec![
                    "commit".into(),
                    "--only".into(),
                    "-m".into(),
                    message.clone(),
                    "--".into(),
                ];
                commit.extend(files.iter().cloned());
                self.checked(commit, true).await?;
                self.snapshot.head_after = Some(self.resolve("HEAD").await?);
            }
            GitAction::Checkout { reference, detach } => {
                self.stage("checking_checkout").await?;
                let target = if *detach {
                    self.resolve(reference).await?
                } else {
                    self.branch_name(reference).await?;
                    reference.clone()
                };
                self.stage("switching_reference").await?;
                self.snapshot.index_may_have_changed = true;
                let mut args = vec!["switch".into(), "--no-guess".into()];
                if *detach {
                    args.push("--detach".into());
                }
                args.extend(["--".into(), target]);
                self.checked(args, true).await?;
                self.snapshot.index_may_have_changed = true;
                self.snapshot.head_after = Some(self.resolve("HEAD").await?);
            }
            GitAction::Fetch { remote, branch } => {
                if let Some(b) = branch {
                    self.branch_name(b).await?;
                }
                self.stage("fetching").await?;
                let mut args = vec!["fetch".into(), "--".into(), remote.clone()];
                if let Some(b) = branch {
                    args.push(b.clone());
                }
                self.checked(args, true).await?;
                self.snapshot.head_after = self.snapshot.head.clone();
            }
            GitAction::Pull {
                remote,
                branch,
                strategy,
            } => {
                self.branch_name(branch).await?;
                if self.snapshot.unborn || self.snapshot.detached {
                    return Err("pull requires an existing checked-out branch"
                        .to_owned()
                        .into());
                }
                let dirty = self
                    .checked(
                        vec!["status".into(), "--porcelain=v1".into(), "-z".into()],
                        false,
                    )
                    .await?;
                if !dirty.stdout.is_empty() {
                    return Err(
                        "pull requires a clean index and working tree; no automatic stash"
                            .to_owned()
                            .into(),
                    );
                }
                self.stage("pulling").await?;
                self.snapshot.index_may_have_changed = true;
                let strategy = match strategy {
                    GitPullStrategy::FfOnly => vec!["--ff-only", "--no-rebase"],
                    GitPullStrategy::Merge => vec!["--no-rebase", "--no-edit", "--ff"],
                    GitPullStrategy::Rebase => vec!["--rebase"],
                };
                let mut args = vec!["pull".into(), "--no-autostash".into()];
                args.extend(strategy.into_iter().map(str::to_owned));
                args.extend(["--".into(), remote.clone(), branch.clone()]);
                self.checked(args, true).await?;
                self.snapshot.index_may_have_changed = true;
                self.snapshot.head_after = Some(self.resolve("HEAD").await?);
            }
            GitAction::Push {
                remote,
                branch,
                force,
            } => {
                self.branch_name(branch).await?;
                let reference = format!("refs/heads/{branch}");
                let target = self.resolve(&reference).await?;
                self.snapshot.start_commit = Some(target.clone());
                self.snapshot.remote_reference = Some(reference.clone());
                self.stage("checking_remote_reference").await?;
                let prior = self
                    .checked(
                        vec![
                            "ls-remote".into(),
                            "--heads".into(),
                            "--".into(),
                            remote.clone(),
                            reference.clone(),
                        ],
                        false,
                    )
                    .await?;
                let old = text(&prior.stdout)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                if !old.is_empty() && !valid_oid(&old) {
                    return Err("invalid remote reference response".to_owned().into());
                }
                self.snapshot.remote_oid_before = (!old.is_empty()).then_some(old.clone());
                self.stage("pushing").await?;
                let mut args = vec!["push".into(), "--porcelain".into()];
                if *force {
                    args.push(format!("--force-with-lease={reference}:{old}"));
                }
                args.extend(["--".into(), remote.clone(), format!("{target}:{reference}")]);
                self.checked(args, true).await?;
                self.snapshot.head_after = self.snapshot.head.clone();
            }
        }
        self.snapshot.phase = "completed".into();
        if !q.is_mutation() {
            self.snapshot.command_completed = true;
            self.snapshot.exit_code = Some(0);
        }
        Ok(())
    }
}
fn valid_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn path_text(bytes: &[u8]) -> (String, Option<String>) {
    (
        String::from_utf8_lossy(bytes).into(),
        std::str::from_utf8(bytes)
            .is_err()
            .then(|| hex::encode(bytes)),
    )
}
fn parse_status(bytes: &[u8]) -> Result<Vec<GitStatusEntry>, String> {
    if !bytes.is_empty() && !bytes.ends_with(&[0]) {
        return Err("truncated Git status records".into());
    }
    let mut parts = bytes.split(|b| *b == 0).filter(|p| !p.is_empty());
    let mut entries = vec![];
    while let Some(p) = parts.next() {
        if p.len() < 4 || p[2] != b' ' {
            return Err("invalid Git status record".into());
        }
        let (path, path_hex) = path_text(&p[3..]);
        let (previous_path, previous_path_hex) = if p[..2].iter().any(|c| b"RC".contains(c)) {
            let (s, h) = path_text(parts.next().ok_or("missing renamed source")?);
            (Some(s), h)
        } else {
            (None, None)
        };
        let xy = &p[..2];
        entries.push(GitStatusEntry {
            index: (xy[0] as char).to_string(),
            worktree: (xy[1] as char).to_string(),
            path,
            path_hex,
            previous_path,
            previous_path_hex,
            conflict: xy.contains(&b'U') || xy == b"AA" || xy == b"DD",
            untracked: xy == b"??",
        });
    }
    Ok(entries)
}
fn parse_log(bytes: &[u8]) -> Result<Vec<GitCommitInfo>, String> {
    let mut parts = bytes.split(|b| *b == 0);
    let mut commits = vec![];
    while let Some(id) = parts.next() {
        let id = text(id).trim_start_matches(['\r', '\n']).to_owned();
        if id.is_empty() {
            continue;
        }
        if !valid_oid(&id) {
            return Err("invalid Git log commit ID".into());
        }
        let parents = text(parts.next().ok_or("incomplete log parents")?)
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if parents.iter().any(|p| !valid_oid(p)) {
            return Err("invalid Git log parent".into());
        }
        let author = text(parts.next().ok_or("incomplete log author")?);
        let email = text(parts.next().ok_or("incomplete log email")?);
        let timestamp_seconds = text(parts.next().ok_or("incomplete log time")?)
            .parse()
            .map_err(|_| "invalid Git log timestamp")?;
        let subject = text(parts.next().ok_or("incomplete log subject")?);
        commits.push(GitCommitInfo {
            id,
            parents,
            author,
            email,
            timestamp_seconds,
            subject,
        });
    }
    Ok(commits)
}
fn bounded(mut r: SystemQueryReply, mut s: GitSnapshot) -> SystemQueryReply {
    loop {
        r.returned_count = (s.entries.len() + s.commits.len()) as u32;
        r.data = Some(SystemQueryData::Git {
            snapshot: s.clone(),
        });
        if serde_json::to_vec(&r).is_ok_and(|b| b.len() <= MAX_SYSTEM_REPLY_BYTES) {
            return r;
        }
        r.truncated = true;
        r.stop_reason = Some("result_byte_budget".into());
        if s.entries.pop().is_some() {
            continue;
        }
        if s.commits.pop().is_some() {
            s.next_skip =
                (!s.commits.is_empty()).then(|| s.log_skip.unwrap_or(0) + s.commits.len() as u32);
            if s.commits.is_empty() {
                r.state = "failed".into();
                r.error = Some(
                    "one commit exceeds the result byte budget; cannot return a usable log page"
                        .into(),
                );
            }
            continue;
        }
        if let Some(diff) = s.diff.as_mut().filter(|d| !d.is_empty()) {
            cut(diff, diff.len() / 2);
            continue;
        }
        for field in [&mut s.repo, &mut s.git_dir] {
            cut(field, 1024);
        }
        if let Some(e) = &mut r.error {
            cut(e, 1024);
        }
        r.data = Some(SystemQueryData::Git { snapshot: s });
        return r;
    }
}
#[cfg(test)]
pub(super) async fn query(
    id: RequestId,
    q: &GitQuery,
    cancelled: watch::Receiver<bool>,
    store: Option<TaskStore>,
) -> SystemQueryReply {
    query_with_sink(id, q, cancelled, GitSink::Store(store), None).await
}

pub(crate) async fn query_with_sink(
    id: RequestId,
    q: &GitQuery,
    cancelled: watch::Receiver<bool>,
    sink: GitSink,
    execution_context: Option<ExecutionContext>,
) -> SystemQueryReply {
    let system = SystemQuery::Git { query: q.clone() };
    let mut r = SystemQueryReply::pending(id, &system);
    r.execution_context = execution_context;
    r.sampled_from_unix_ms = Some(now());
    let mut runner = Runner {
        program: PathBuf::from("git"),
        cwd: PathBuf::from(&q.repo),
        deadline: Instant::now() + Duration::from_millis(u64::from(q.timeout_ms)),
        cancelled,
        r: r.clone(),
        snapshot: GitSnapshot::default(),
        sink,
        effects_started: false,
    };
    let result = if let Err(e) = q.validate() {
        Err(e.to_owned().into())
    } else if !Path::new(&q.repo).is_absolute() {
        Err("repo must be an absolute path on the target OS"
            .to_owned()
            .into())
    } else {
        runner.perform(q).await
    };
    r = runner.r;
    r.sampled_at_unix_ms = Some(now());
    r.state = "completed".into();
    if let Err(e) = result {
        r.state = if e.unconfirmed {
            "unconfirmed"
        } else if e.cancelled {
            "cancelled"
        } else {
            "failed"
        }
        .into();
        r.error = Some(e.message);
        if runner.snapshot.phase.is_empty() {
            runner.snapshot.phase = r.state.clone();
        }
    }
    bounded(r, runner.snapshot)
}

/// Resolve a lost push receipt only from the exact originally selected object/reference.
/// A matching remote ref establishes the desired state, not which process published it.
pub(crate) async fn reconcile_push(
    q: &GitQuery,
    original: &SystemQueryReply,
) -> Option<SystemQueryReply> {
    let GitAction::Push { remote, branch, .. } = &q.action else {
        return None;
    };
    let Some(SystemQueryData::Git { snapshot }) = &original.data else {
        return None;
    };
    let expected = snapshot.start_commit.as_ref()?;
    let reference = snapshot.remote_reference.as_ref()?;
    if reference != &format!("refs/heads/{branch}") || !valid_oid(expected) {
        return None;
    }
    let (_send, cancelled) = watch::channel(false);
    let mut runner = Runner {
        program: PathBuf::from("git"),
        cwd: PathBuf::from(&snapshot.repo),
        deadline: Instant::now() + Duration::from_secs(3),
        cancelled,
        r: original.clone(),
        snapshot: snapshot.clone(),
        sink: GitSink::Store(None),
        effects_started: false,
    };
    let out = runner
        .checked(
            vec![
                "ls-remote".into(),
                "--heads".into(),
                "--".into(),
                remote.clone(),
                reference.clone(),
            ],
            false,
        )
        .await
        .ok()?;
    let oid = text(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_owned();
    if !oid.is_empty() && !valid_oid(&oid) {
        return None;
    }
    let mut updated = original.clone();
    let mut data = snapshot.clone();
    data.remote_oid_after = (!oid.is_empty()).then_some(oid.clone());
    data.remote_observed_at_unix_ms = Some(now());
    if &oid == expected {
        updated.state = "completed".into();
        updated.error = None;
        data.phase = "remote_reference_observed".into();
        updated.warnings.push("desired remote reference observed; this does not attribute publication to this particular Git process".into());
    } else {
        updated.error=Some("remote reference differs from the originally selected commit; no replay, force-push or rollback performed".into());
    }
    Some(bounded(updated, data))
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
