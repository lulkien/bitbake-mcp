//! Server-owned command jobs that survive a client request timing out.
//!
//! The table owns only detached jobs after their initial tool call returns.
//! Execution and bookkeeping are separate Tokio tasks so cancellation of a
//! `job_wait` or `job_cancel` request cannot orphan a job transition.

use crate::exec::ExecResult;
use serde_json::json;
use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::AbortHandle};

pub(crate) const DETACH_AFTER_MS: u64 = 50_000;
pub(crate) const FINISHED_KEEP: usize = 16;

#[derive(Clone, Default, Debug)]
pub(crate) struct JobTable(Arc<Mutex<Inner>>);

#[derive(Default, Debug)]
struct Inner {
    next_id: u64,
    jobs: BTreeMap<u64, Job>,
}

#[derive(Debug)]
struct Job {
    key: PathBuf,
    build_dir: String,
    command: String,
    started: Instant,
    log: Arc<OnceLock<PathBuf>>,
    detached: bool,
    tx: watch::Sender<Option<Arc<Done>>>,
    state: State,
}

#[derive(Debug)]
enum State {
    Running(AbortHandle),
    Finished(Arc<Done>),
}

#[derive(Debug)]
pub(crate) struct Done {
    pub(crate) json: String,
    pub(crate) run_ms: u128,
    pub(crate) cancelled: bool,
    pub(crate) command: String,
}

pub(crate) struct Started {
    pub(crate) id: u64,
    pub(crate) done: watch::Receiver<Option<Arc<Done>>>,
    pub(crate) guard: DetachGuard,
    pub(crate) log: Arc<OnceLock<PathBuf>>,
    pub(crate) command: String,
    pub(crate) build_dir: String,
    pub(crate) started: Instant,
}

pub(crate) struct DetachGuard {
    table: JobTable,
    id: u64,
    armed: bool,
}

pub(crate) struct JobInfo {
    pub(crate) id: u64,
    pub(crate) build_dir: String,
    pub(crate) command: String,
    pub(crate) log: Option<PathBuf>,
    pub(crate) run_ms: u128,
    started: Instant,
}

pub(crate) enum JobStatus {
    Running(JobInfo),
    Finished(Arc<Done>),
}

impl JobTable {
    pub(crate) fn start<F>(
        &self,
        key: PathBuf,
        build_dir: String,
        command: String,
        log: Arc<OnceLock<PathBuf>>,
        future: F,
    ) -> Result<Started, u64>
    where
        F: Future<Output = ExecResult> + Send + 'static,
    {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((&id, _)) = inner.jobs.iter().find(|(_, job)| {
            job.detached && job.key == key && matches!(job.state, State::Running(_))
        }) {
            return Err(id);
        }

        if inner.next_id == 0 {
            inner.next_id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
        }
        inner.next_id = inner.next_id.saturating_add(1).max(1);
        let id = inner.next_id;
        let started = Instant::now();
        let (tx, done) = watch::channel(None);
        let table = self.clone();
        let worker_command = command.clone();
        let worker = tokio::spawn(future);
        let abort = worker.abort_handle();
        let worker_tx = tx.clone();
        tokio::spawn(async move {
            let done = match worker.await {
                Ok(result) => Done {
                    json: serde_json::to_string(&result).unwrap_or_default(),
                    run_ms: started.elapsed().as_millis(),
                    cancelled: false,
                    command: worker_command.clone(),
                },
                Err(error) if error.is_cancelled() => Done {
                    json: String::new(),
                    run_ms: started.elapsed().as_millis(),
                    cancelled: true,
                    command: worker_command.clone(),
                },
                Err(error) => Done {
                    json: json!({"ok":false,"error_code":"BUILD_FAILURE",
                        "message":format!("job task failed: {error}"),"command":worker_command})
                    .to_string(),
                    run_ms: started.elapsed().as_millis(),
                    cancelled: false,
                    command: worker_command,
                },
            };
            table.finish(id, Arc::new(done), &worker_tx);
        });
        inner.jobs.insert(
            id,
            Job {
                key,
                build_dir: build_dir.clone(),
                command: command.clone(),
                started,
                log: log.clone(),
                detached: false,
                tx,
                state: State::Running(abort),
            },
        );
        drop(inner);

        Ok(Started {
            id,
            done,
            guard: DetachGuard {
                table: self.clone(),
                id,
                armed: true,
            },
            log,
            command,
            build_dir,
            started,
        })
    }

    fn finish(&self, id: u64, done: Arc<Done>, tx: &watch::Sender<Option<Arc<Done>>>) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = inner.jobs.get_mut(&id) else {
            tx.send_replace(Some(done));
            return;
        };
        if let State::Finished(existing) = &job.state {
            tx.send_replace(Some(existing.clone()));
            return;
        }
        job.state = State::Finished(done.clone());
        tx.send_replace(Some(done));

        evict_finished(&mut inner);
    }

    pub(crate) fn mark_detached(&self, id: u64) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = inner.jobs.get_mut(&id) {
            job.detached = true;
        }
        evict_finished(&mut inner);
    }

    pub(crate) fn remove(&self, id: u64) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .remove(&id);
    }

    pub(crate) async fn wait(&self, id: u64, duration: Duration) -> Option<JobStatus> {
        let (mut done_rx, info) = {
            let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let job = inner.jobs.get(&id)?;
            match &job.state {
                State::Finished(done) => return Some(JobStatus::Finished(done.clone())),
                State::Running(_) => {
                    let rx = job.tx.subscribe();
                    let info = JobInfo {
                        id,
                        build_dir: job.build_dir.clone(),
                        command: job.command.clone(),
                        log: job.log.get().cloned(),
                        run_ms: job.started.elapsed().as_millis(),
                        started: job.started,
                    };
                    (rx, info)
                }
            }
        };
        if let Ok(Ok(value)) =
            tokio::time::timeout(duration, done_rx.wait_for(Option::is_some)).await
            && let Some(done) = value.as_ref()
        {
            return Some(JobStatus::Finished(done.clone()));
        }
        let mut info = info;
        info.run_ms = info.started.elapsed().as_millis();
        Some(JobStatus::Running(info))
    }

    pub(crate) async fn cancel(&self, id: u64) -> Option<Arc<Done>> {
        let (abort, mut done_rx) = {
            let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let job = inner.jobs.get(&id)?;
            match &job.state {
                State::Finished(done) => return Some(done.clone()),
                State::Running(abort) => (abort.clone(), job.tx.subscribe()),
            }
        };
        abort.abort();
        if let Ok(value) = done_rx.wait_for(Option::is_some).await {
            value.as_ref().cloned()
        } else {
            None
        }
    }

    pub(crate) async fn shutdown(&self) {
        let mut receivers = Vec::new();
        {
            let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            for job in inner.jobs.values() {
                if let State::Running(abort) = &job.state {
                    let receiver = job.tx.subscribe();
                    abort.abort();
                    receivers.push(receiver);
                }
            }
        }
        for mut receiver in receivers {
            let _ = receiver.wait_for(Option::is_some).await;
        }
    }
}

fn evict_finished(inner: &mut Inner) {
    let finished: Vec<u64> = inner
        .jobs
        .iter()
        .filter_map(|(&id, job)| {
            (job.detached && matches!(job.state, State::Finished(_))).then_some(id)
        })
        .collect();
    let evict = finished.len().saturating_sub(FINISHED_KEEP);
    for old in finished.into_iter().take(evict) {
        inner.jobs.remove(&old);
    }
}

impl DetachGuard {
    pub(crate) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for DetachGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let job = self
            .table
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .remove(&self.id);
        if let Some(job) = job {
            if let State::Running(abort) = job.state {
                abort.abort();
            }
            job.tx.send_replace(Some(Arc::new(Done {
                json: String::new(),
                run_ms: job.started.elapsed().as_millis(),
                cancelled: true,
                command: job.command,
            })));
        }
    }
}
