//! Job scheduler implementation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, watch};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use winter_atproto::{AtprotoClient, JOB_COLLECTION, Tid};

use crate::{Job, JobSchedule, JobStatus, SchedulerError};

/// Minimum sleep duration between scheduler checks.
const MIN_SLEEP_SECS: u64 = 1;

/// Maximum sleep duration between scheduler checks.
const MAX_SLEEP_SECS: u64 = 60;

/// Type alias for the job executor function.
pub type JobExecutor =
    Box<dyn Fn(Job) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// The job scheduler.
pub struct Scheduler {
    client: AtprotoClient,
    jobs: Arc<RwLock<Vec<Job>>>,
}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new(client: AtprotoClient) -> Self {
        Self {
            client,
            jobs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Load jobs from PDS.
    #[tracing::instrument(skip(self))]
    pub async fn load_jobs(&self) -> Result<(), SchedulerError> {
        let records = self
            .client
            .list_all_records::<winter_atproto::Job>(JOB_COLLECTION)
            .await?;

        let mut jobs = Vec::new();
        for record in records {
            // Parse rkey from URI
            let rkey = record
                .uri
                .rsplit('/')
                .next()
                .unwrap_or(&record.uri)
                .to_string();

            let job = Job {
                rkey,
                name: record.value.name,
                instructions: record.value.instructions,
                schedule: match record.value.schedule {
                    winter_atproto::JobSchedule::Once { at } => JobSchedule::Once { at },
                    winter_atproto::JobSchedule::Interval { seconds } => {
                        JobSchedule::Interval { seconds }
                    }
                },
                status: match record.value.status {
                    winter_atproto::JobStatus::Pending => JobStatus::Pending,
                    winter_atproto::JobStatus::Running => JobStatus::Interrupted, // Was running when we stopped
                    winter_atproto::JobStatus::Completed => JobStatus::Completed,
                    winter_atproto::JobStatus::Failed { error } => JobStatus::Failed { error },
                },
                last_run: record.value.last_run,
                next_run: record.value.next_run.unwrap_or_else(Utc::now),
                failure_count: record.value.failure_count,
                created_at: record.value.created_at,
            };
            jobs.push(job);
        }

        info!(count = jobs.len(), "loaded jobs from PDS");
        *self.jobs.write().await = jobs;
        Ok(())
    }

    /// Add a new job.
    #[tracing::instrument(skip(self), fields(rkey = %job.rkey, name = %job.name))]
    pub async fn add_job(&self, job: Job) -> Result<(), SchedulerError> {
        // Check for duplicates
        {
            let jobs = self.jobs.read().await;
            if jobs.iter().any(|j| j.rkey == job.rkey) {
                return Err(SchedulerError::JobExists(job.rkey));
            }
        }

        // Create record in PDS
        let record = winter_atproto::Job {
            name: job.name.clone(),
            instructions: job.instructions.clone(),
            schedule: match &job.schedule {
                JobSchedule::Once { at } => winter_atproto::JobSchedule::Once { at: *at },
                JobSchedule::Interval { seconds } => {
                    winter_atproto::JobSchedule::Interval { seconds: *seconds }
                }
            },
            status: winter_atproto::JobStatus::Pending,
            last_run: None,
            next_run: Some(job.next_run),
            failure_count: 0,
            created_at: job.created_at,
        };

        self.client
            .create_record(JOB_COLLECTION, Some(&job.rkey), &record)
            .await?;

        // Add to local state
        self.jobs.write().await.push(job);

        Ok(())
    }

    /// Cancel a job.
    pub async fn cancel_job(&self, rkey: &str) -> Result<(), SchedulerError> {
        // Get job name before removal for logging
        let job_name = {
            let jobs = self.jobs.read().await;
            jobs.iter().find(|j| j.rkey == rkey).map(|j| j.name.clone())
        };

        // Remove from PDS
        self.client.delete_record(JOB_COLLECTION, rkey).await?;

        // Remove from local state
        let mut jobs = self.jobs.write().await;
        jobs.retain(|j| j.rkey != rkey);

        info!(rkey, name = ?job_name, "cancelled job");
        Ok(())
    }

    /// List all jobs.
    pub async fn list_jobs(&self) -> Vec<Job> {
        self.jobs.read().await.clone()
    }

    /// Get a job by rkey.
    pub async fn get_job(&self, rkey: &str) -> Option<Job> {
        self.jobs
            .read()
            .await
            .iter()
            .find(|j| j.rkey == rkey)
            .cloned()
    }

    /// Get a job by name.
    pub async fn get_job_by_name(&self, name: &str) -> Option<Job> {
        self.jobs
            .read()
            .await
            .iter()
            .find(|j| j.name == name)
            .cloned()
    }

    /// Reset a failed or interrupted job to pending status with immediate next_run.
    /// Returns true if the job was reset, false if not found or not in a resettable state.
    pub async fn reset_job(&self, rkey: &str) -> Result<bool, SchedulerError> {
        let needs_reset = {
            let jobs = self.jobs.read().await;
            jobs.iter()
                .find(|j| j.rkey == rkey)
                .map(|j| matches!(j.status, JobStatus::Failed { .. } | JobStatus::Interrupted))
                .unwrap_or(false)
        };

        if !needs_reset {
            return Ok(false);
        }

        // Update local state
        {
            let mut jobs = self.jobs.write().await;
            if let Some(job) = jobs.iter_mut().find(|j| j.rkey == rkey) {
                job.status = JobStatus::Pending;
                job.next_run = Utc::now();
                job.failure_count = 0;
            }
        }

        // Sync to PDS
        self.sync_job_to_pds(rkey).await?;
        info!(rkey, "reset job to pending");
        Ok(true)
    }

    /// Delete all jobs with the given name except the most recent one.
    /// Returns the number of jobs deleted.
    pub async fn deduplicate_jobs_by_name(&self, name: &str) -> Result<usize, SchedulerError> {
        let mut jobs_to_delete = Vec::new();

        {
            let jobs = self.jobs.read().await;
            let mut matching: Vec<_> = jobs.iter().filter(|j| j.name == name).collect();

            if matching.len() <= 1 {
                return Ok(0);
            }

            // Sort by created_at descending (newest first)
            matching.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            // Keep the first (newest), delete the rest
            for job in matching.into_iter().skip(1) {
                jobs_to_delete.push(job.rkey.clone());
            }
        }

        let count = jobs_to_delete.len();
        for rkey in jobs_to_delete {
            self.cancel_job(&rkey).await?;
            info!(rkey = %rkey, name = %name, "deleted duplicate job");
        }

        Ok(count)
    }

    /// Schedule a new one-shot job.
    pub async fn schedule_once(
        &self,
        name: String,
        instructions: String,
        at: chrono::DateTime<Utc>,
    ) -> Result<String, SchedulerError> {
        let rkey = Tid::now().to_string();
        let job = Job::once(rkey.clone(), name, instructions, at);
        self.add_job(job).await?;
        Ok(rkey)
    }

    /// Schedule a new recurring job.
    pub async fn schedule_recurring(
        &self,
        name: String,
        instructions: String,
        interval_seconds: u64,
    ) -> Result<String, SchedulerError> {
        let rkey = Tid::now().to_string();
        let job = Job::interval(rkey.clone(), name, instructions, interval_seconds);
        self.add_job(job).await?;
        Ok(rkey)
    }

    /// Run the scheduler loop.
    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>, executor: JobExecutor) {
        info!("scheduler starting");

        loop {
            // Check for shutdown
            if *shutdown_rx.borrow() {
                info!("scheduler shutting down");
                break;
            }

            // Find and execute due jobs
            let due_jobs = self.get_due_jobs().await;
            for job in due_jobs {
                if *shutdown_rx.borrow() {
                    info!("shutdown requested, not starting new jobs");
                    break;
                }

                self.execute_job(job, &executor).await;
            }

            // Calculate sleep duration
            let sleep_duration = self.calculate_sleep_duration().await;

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("scheduler received shutdown signal");
                    }
                }
                _ = sleep(sleep_duration) => {}
            }
        }

        info!("scheduler shut down gracefully");
    }

    /// Get all jobs that are due to run.
    async fn get_due_jobs(&self) -> Vec<Job> {
        self.jobs
            .read()
            .await
            .iter()
            .filter(|j| j.is_due())
            .cloned()
            .collect()
    }

    /// Take the first due job and mark it as running.
    ///
    /// Returns `None` if no jobs are due.
    pub async fn take_due_job(&self) -> Option<Job> {
        let job = {
            let jobs = self.jobs.read().await;
            jobs.iter().find(|j| j.is_due()).cloned()
        };

        if let Some(ref job) = job {
            self.update_job_status(&job.rkey, JobStatus::Running).await;
        }

        job
    }

    /// Sleep until the next job is due.
    ///
    /// Returns immediately if a job is already due.
    /// Sleeps for at most MAX_SLEEP_SECS (60 seconds) before returning.
    pub async fn sleep_until_next_job(&self) {
        let duration = self.calculate_sleep_duration().await;
        sleep(duration).await;
    }

    /// Calculate how long to sleep until the next job is due.
    pub async fn calculate_sleep_duration(&self) -> std::time::Duration {
        let jobs = self.jobs.read().await;
        let now = Utc::now();

        let next_due = jobs
            .iter()
            .filter(|j| {
                j.status == JobStatus::Pending
                    || (matches!(j.status, JobStatus::Failed { .. })
                        && matches!(j.schedule, JobSchedule::Interval { .. }))
            })
            .map(|j| j.next_run)
            .min();

        let secs = match next_due {
            Some(next) => {
                let diff = (next - now).num_seconds();
                (diff.max(MIN_SLEEP_SECS as i64) as u64).min(MAX_SLEEP_SECS)
            }
            None => MAX_SLEEP_SECS,
        };

        std::time::Duration::from_secs(secs)
    }

    /// Execute a single job.
    ///
    /// This method handles running, success/failure status updates, rescheduling, and PDS sync.
    #[tracing::instrument(skip(self, executor), fields(rkey = %job.rkey, name = %job.name))]
    pub async fn execute_job(&self, job: Job, executor: &JobExecutor) {
        info!(rkey = %job.rkey, name = %job.name, "executing job");

        // Mark as running
        self.update_job_status(&job.rkey, JobStatus::Running).await;

        // Execute
        let result = executor(job.clone()).await;

        // Update state based on result
        match result {
            Ok(()) => {
                let now = Utc::now();
                let mut jobs = self.jobs.write().await;
                if let Some(j) = jobs.iter_mut().find(|j| j.rkey == job.rkey) {
                    j.last_run = Some(now);
                    j.failure_count = 0;

                    match &j.schedule {
                        JobSchedule::Once { .. } => {
                            j.status = JobStatus::Completed;
                            info!(rkey = %job.rkey, "one-shot job completed");
                        }
                        JobSchedule::Interval { seconds } => {
                            j.next_run = now + chrono::Duration::seconds(*seconds as i64);
                            j.status = JobStatus::Pending;
                            debug!(rkey = %job.rkey, next_run = %j.next_run, "rescheduled interval job");
                        }
                    }
                }
            }
            Err(error) => {
                let mut jobs = self.jobs.write().await;
                if let Some(j) = jobs.iter_mut().find(|j| j.rkey == job.rkey) {
                    j.failure_count += 1;
                    let retry_delay = j.calculate_retry_delay();

                    if matches!(j.schedule, JobSchedule::Interval { .. }) {
                        j.next_run = Utc::now() + retry_delay;
                        j.status = JobStatus::Failed {
                            error: error.clone(),
                        };
                        warn!(
                            rkey = %job.rkey,
                            failure_count = j.failure_count,
                            next_retry = %j.next_run,
                            error = %error,
                            "interval job failed, scheduled retry"
                        );
                    } else {
                        j.status = JobStatus::Failed {
                            error: error.clone(),
                        };
                        error!(rkey = %job.rkey, error = %error, "one-shot job failed");
                    }
                }
            }
        }

        // Sync to PDS
        if let Err(e) = self.sync_job_to_pds(&job.rkey).await {
            error!(rkey = %job.rkey, error = %e, "failed to sync job to PDS");
        }
    }

    /// Update a job's status in local state.
    async fn update_job_status(&self, rkey: &str, status: JobStatus) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.iter_mut().find(|j| j.rkey == rkey) {
            job.status = status;
        }
    }

    /// Sync a job's state to PDS.
    async fn sync_job_to_pds(&self, rkey: &str) -> Result<(), SchedulerError> {
        let jobs = self.jobs.read().await;
        let job = jobs
            .iter()
            .find(|j| j.rkey == rkey)
            .ok_or_else(|| SchedulerError::JobNotFound(rkey.to_string()))?;

        let record = winter_atproto::Job {
            name: job.name.clone(),
            instructions: job.instructions.clone(),
            schedule: match &job.schedule {
                JobSchedule::Once { at } => winter_atproto::JobSchedule::Once { at: *at },
                JobSchedule::Interval { seconds } => {
                    winter_atproto::JobSchedule::Interval { seconds: *seconds }
                }
            },
            status: match &job.status {
                JobStatus::Pending => winter_atproto::JobStatus::Pending,
                JobStatus::Running => winter_atproto::JobStatus::Running,
                JobStatus::Completed => winter_atproto::JobStatus::Completed,
                JobStatus::Failed { error } => winter_atproto::JobStatus::Failed {
                    error: error.clone(),
                },
                JobStatus::Interrupted => winter_atproto::JobStatus::Pending, // Treat as pending on restart
            },
            last_run: job.last_run,
            next_run: Some(job.next_run),
            failure_count: job.failure_count,
            created_at: job.created_at,
        };

        self.client
            .put_record(JOB_COLLECTION, rkey, &record)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sleep_bounds() {
        assert_eq!(MIN_SLEEP_SECS, 1);
        assert_eq!(MAX_SLEEP_SECS, 60);
        assert!(MIN_SLEEP_SECS < MAX_SLEEP_SECS);
    }

    #[test]
    fn test_job_schedule_once() {
        let at = Utc::now() + chrono::Duration::hours(1);
        let job = Job::once(
            "test-rkey".to_string(),
            "Test Job".to_string(),
            "Do something".to_string(),
            at,
        );

        assert_eq!(job.rkey, "test-rkey");
        assert_eq!(job.name, "Test Job");
        assert_eq!(job.instructions, "Do something");
        assert!(matches!(job.schedule, JobSchedule::Once { .. }));
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.failure_count, 0);
        assert!(job.last_run.is_none());
    }

    #[test]
    fn test_job_schedule_interval() {
        let job = Job::interval(
            "test-rkey".to_string(),
            "Recurring Job".to_string(),
            "Do something repeatedly".to_string(),
            3600,
        );

        assert_eq!(job.rkey, "test-rkey");
        assert_eq!(job.name, "Recurring Job");
        assert!(matches!(
            job.schedule,
            JobSchedule::Interval { seconds: 3600 }
        ));
        assert_eq!(job.status, JobStatus::Pending);
    }

    #[test]
    fn test_job_is_due_future_once() {
        let at = Utc::now() + chrono::Duration::hours(1);
        let job = Job::once(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            at,
        );

        // Job scheduled for the future should not be due
        assert!(!job.is_due());
    }

    #[test]
    fn test_job_is_due_past_once() {
        let at = Utc::now() - chrono::Duration::hours(1);
        let job = Job::once(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            at,
        );

        // Job scheduled for the past should be due
        assert!(job.is_due());
    }

    #[test]
    fn test_job_calculate_next_run_once() {
        let at = Utc::now();
        let job = Job::once(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            at,
        );

        // One-shot job should not have a next run after execution
        assert!(job.calculate_next_run().is_none());
    }

    #[test]
    fn test_job_calculate_next_run_interval() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            60,
        );

        job.last_run = Some(Utc::now());
        let next = job.calculate_next_run();

        assert!(next.is_some());
        // Next run should be approximately 60 seconds after last_run
        let diff = (next.unwrap() - job.last_run.unwrap()).num_seconds();
        assert_eq!(diff, 60);
    }

    #[test]
    fn test_job_status_default() {
        let status: JobStatus = Default::default();
        assert_eq!(status, JobStatus::Pending);
    }

    #[test]
    fn test_job_status_failed_with_error() {
        let status = JobStatus::Failed {
            error: "Something went wrong".to_string(),
        };

        match status {
            JobStatus::Failed { error } => assert_eq!(error, "Something went wrong"),
            _ => panic!("Expected Failed status"),
        }
    }
}
