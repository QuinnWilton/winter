//! Scheduler types.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Record key (TID).
    pub rkey: String,
    /// Human-readable name for the job.
    pub name: String,
    /// Instructions for the agent to execute.
    pub instructions: String,
    /// When/how often to run this job.
    pub schedule: JobSchedule,
    /// Current status of the job.
    pub status: JobStatus,
    /// When this job last ran successfully.
    pub last_run: Option<DateTime<Utc>>,
    /// When this job should next run (or retry after failure).
    pub next_run: DateTime<Utc>,
    /// Number of consecutive failures (resets on success).
    pub failure_count: u32,
    /// When this job was created.
    pub created_at: DateTime<Utc>,
}

/// How a job is scheduled to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobSchedule {
    /// Run once at a specific time, then mark completed.
    Once { at: DateTime<Utc> },
    /// Run every N seconds from last_run.
    Interval { seconds: u64 },
}

/// Current status of a job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Job is waiting to run.
    #[default]
    Pending,
    /// Job is currently executing.
    Running,
    /// Job completed successfully (for one-shot jobs).
    Completed,
    /// Job failed with an error.
    Failed { error: String },
    /// Job was interrupted (e.g., server shutdown during execution).
    Interrupted,
}

impl Job {
    /// Create a new one-shot job.
    pub fn once(rkey: String, name: String, instructions: String, at: DateTime<Utc>) -> Self {
        Self {
            rkey,
            name,
            instructions,
            schedule: JobSchedule::Once { at },
            status: JobStatus::Pending,
            last_run: None,
            next_run: at,
            failure_count: 0,
            created_at: Utc::now(),
        }
    }

    /// Create a new recurring interval job.
    pub fn interval(rkey: String, name: String, instructions: String, seconds: u64) -> Self {
        Self {
            rkey,
            name,
            instructions,
            schedule: JobSchedule::Interval { seconds },
            status: JobStatus::Pending,
            last_run: None,
            next_run: Utc::now(),
            failure_count: 0,
            created_at: Utc::now(),
        }
    }

    /// Check if this job is due to run.
    pub fn is_due(&self) -> bool {
        let now = Utc::now();
        match &self.status {
            JobStatus::Pending => self.next_run <= now,
            // Interrupted jobs should run immediately after restart
            JobStatus::Interrupted => true,
            // Failed interval jobs can retry after next_run time
            JobStatus::Failed { .. } => {
                matches!(self.schedule, JobSchedule::Interval { .. }) && self.next_run <= now
            }
            _ => false,
        }
    }

    /// Calculate the next run time after a successful execution.
    pub fn calculate_next_run(&self) -> Option<DateTime<Utc>> {
        match &self.schedule {
            JobSchedule::Once { .. } => None,
            JobSchedule::Interval { seconds } => {
                let base = self.last_run.unwrap_or_else(Utc::now);
                Some(base + Duration::seconds(*seconds as i64))
            }
        }
    }

    /// Calculate retry delay based on failure count.
    pub fn calculate_retry_delay(&self) -> Duration {
        // Exponential backoff: 5min, 10min, 20min, 40min, max 1hr
        let base_secs = 300i64; // 5 minutes
        let max_secs = 3600i64; // 1 hour
        let backoff = base_secs * (1 << self.failure_count.min(4));
        Duration::seconds(backoff.min(max_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // === Unit Tests ===

    #[test]
    fn test_job_is_due() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Do something".to_string(),
            60,
        );

        // New job should be due immediately
        assert!(job.is_due());

        // Running job should not be due
        job.status = JobStatus::Running;
        assert!(!job.is_due());

        // Completed job should not be due
        job.status = JobStatus::Completed;
        assert!(!job.is_due());
    }

    #[test]
    fn test_retry_delay() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Do something".to_string(),
            60,
        );

        job.failure_count = 0;
        assert_eq!(job.calculate_retry_delay().num_seconds(), 300);

        job.failure_count = 1;
        assert_eq!(job.calculate_retry_delay().num_seconds(), 600);

        job.failure_count = 5;
        assert_eq!(job.calculate_retry_delay().num_seconds(), 3600); // Capped at max
    }

    #[test]
    fn test_job_once_not_due_in_future() {
        let at = Utc::now() + Duration::hours(1);
        let job = Job::once(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            at,
        );
        assert!(!job.is_due());
    }

    #[test]
    fn test_job_interrupted_always_due() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            60,
        );
        job.status = JobStatus::Interrupted;
        job.next_run = Utc::now() + Duration::hours(1); // Even if far in future
        assert!(job.is_due()); // Interrupted jobs should run immediately
    }

    #[test]
    fn test_failed_once_job_not_due() {
        let mut job = Job::once(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            Utc::now() - Duration::hours(1), // In the past
        );
        job.status = JobStatus::Failed {
            error: "failed".to_string(),
        };
        assert!(!job.is_due()); // One-shot jobs don't retry
    }

    #[test]
    fn test_failed_interval_job_due_after_delay() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            60,
        );
        job.status = JobStatus::Failed {
            error: "failed".to_string(),
        };
        job.next_run = Utc::now() - Duration::seconds(1); // Retry time passed
        assert!(job.is_due()); // Should retry

        job.next_run = Utc::now() + Duration::hours(1); // Retry time not reached
        assert!(!job.is_due()); // Should not retry yet
    }

    #[test]
    fn test_calculate_next_run_once_returns_none() {
        let job = Job::once(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            Utc::now(),
        );
        assert!(job.calculate_next_run().is_none());
    }

    #[test]
    fn test_calculate_next_run_interval_uses_last_run() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            3600,
        );
        let last = Utc::now() - Duration::hours(2);
        job.last_run = Some(last);

        let next = job.calculate_next_run().unwrap();
        assert_eq!((next - last).num_seconds(), 3600);
    }

    // === Property-Based Tests ===

    proptest! {
        // Retry delay should always be positive and within bounds
        #[test]
        fn retry_delay_is_bounded(failure_count in 0u32..100) {
            let mut job = Job::interval(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                60,
            );
            job.failure_count = failure_count;

            let delay = job.calculate_retry_delay();
            let secs = delay.num_seconds();

            prop_assert!(secs >= 300, "Retry delay should be at least 5 minutes");
            prop_assert!(secs <= 3600, "Retry delay should be at most 1 hour");
        }

        // Retry delay should be monotonically non-decreasing with failure count
        // (until capped)
        #[test]
        fn retry_delay_non_decreasing(failures_a in 0u32..10, failures_b in 0u32..10) {
            let mut job_a = Job::interval(
                "a".to_string(),
                "A".to_string(),
                "Instructions".to_string(),
                60,
            );
            let mut job_b = job_a.clone();
            job_b.rkey = "b".to_string();
            job_b.name = "B".to_string();

            job_a.failure_count = failures_a;
            job_b.failure_count = failures_b;

            let delay_a = job_a.calculate_retry_delay();
            let delay_b = job_b.calculate_retry_delay();

            if failures_a <= failures_b {
                prop_assert!(
                    delay_a <= delay_b,
                    "More failures should mean longer (or equal) delay: {} failures -> {} secs, {} failures -> {} secs",
                    failures_a, delay_a.num_seconds(),
                    failures_b, delay_b.num_seconds()
                );
            }
        }

        // Interval job should always have a next_run after calculate_next_run
        #[test]
        fn interval_job_always_has_next_run(interval_secs in 1u64..86400) {
            let mut job = Job::interval(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                interval_secs,
            );
            job.last_run = Some(Utc::now());

            let next = job.calculate_next_run();
            prop_assert!(next.is_some(), "Interval job should always have next run");
        }

        // One-shot job should never have a next_run after calculate_next_run
        #[test]
        fn once_job_never_has_next_run(offset_hours in -100i64..100) {
            let at = Utc::now() + Duration::hours(offset_hours);
            let job = Job::once(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                at,
            );

            prop_assert!(
                job.calculate_next_run().is_none(),
                "One-shot job should never have next run"
            );
        }

        // Next run should be exactly interval seconds after last_run
        #[test]
        fn next_run_exact_interval(interval_secs in 1u64..86400) {
            let last_run = Utc::now();
            let mut job = Job::interval(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                interval_secs,
            );
            job.last_run = Some(last_run);

            let next = job.calculate_next_run().unwrap();
            let diff = (next - last_run).num_seconds();

            prop_assert_eq!(
                diff as u64,
                interval_secs,
                "Next run should be exactly interval seconds after last run"
            );
        }

        // Job status determines is_due for non-pending states
        #[test]
        fn completed_job_never_due(next_run_offset in -1000i64..1000) {
            let mut job = Job::interval(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                60,
            );
            job.status = JobStatus::Completed;
            job.next_run = Utc::now() + Duration::seconds(next_run_offset);

            prop_assert!(
                !job.is_due(),
                "Completed job should never be due regardless of next_run"
            );
        }

        // Running job is never due
        #[test]
        fn running_job_never_due(next_run_offset in -1000i64..1000) {
            let mut job = Job::interval(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                60,
            );
            job.status = JobStatus::Running;
            job.next_run = Utc::now() + Duration::seconds(next_run_offset);

            prop_assert!(
                !job.is_due(),
                "Running job should never be due regardless of next_run"
            );
        }

        // Interrupted job is always due regardless of next_run
        #[test]
        fn interrupted_job_always_due(next_run_offset in -1000i64..1000) {
            let mut job = Job::interval(
                "test".to_string(),
                "Test".to_string(),
                "Instructions".to_string(),
                60,
            );
            job.status = JobStatus::Interrupted;
            job.next_run = Utc::now() + Duration::seconds(next_run_offset);

            prop_assert!(
                job.is_due(),
                "Interrupted job should always be due regardless of next_run"
            );
        }
    }

    // === Metamorphic Tests ===

    // Metamorphic: Two interval jobs with same last_run but different intervals
    // should have next_run times that differ by exactly (interval_b - interval_a)
    #[test]
    fn metamorphic_next_run_difference_matches_interval_difference() {
        let last_run = Utc::now();
        let interval_a = 60u64;
        let interval_b = 120u64;

        let mut job_a = Job::interval(
            "a".to_string(),
            "A".to_string(),
            "Instructions".to_string(),
            interval_a,
        );
        job_a.last_run = Some(last_run);

        let mut job_b = Job::interval(
            "b".to_string(),
            "B".to_string(),
            "Instructions".to_string(),
            interval_b,
        );
        job_b.last_run = Some(last_run);

        let next_a = job_a.calculate_next_run().unwrap();
        let next_b = job_b.calculate_next_run().unwrap();

        let diff = (next_b - next_a).num_seconds();
        assert_eq!(
            diff as u64,
            interval_b - interval_a,
            "Difference in next_run should equal difference in intervals"
        );
    }

    // Metamorphic: Retry delay doubles (up to cap) for each failure increment
    #[test]
    fn metamorphic_retry_delay_doubles() {
        let mut job = Job::interval(
            "test".to_string(),
            "Test".to_string(),
            "Instructions".to_string(),
            60,
        );

        let delays: Vec<i64> = (0..5)
            .map(|i| {
                job.failure_count = i;
                job.calculate_retry_delay().num_seconds()
            })
            .collect();

        // Each delay should be double the previous (before hitting cap)
        for i in 1..delays.len() {
            if delays[i - 1] < 3600 {
                assert_eq!(
                    delays[i],
                    (delays[i - 1] * 2).min(3600),
                    "Delay at failure {} should be double (or capped)",
                    i
                );
            }
        }
    }

    // Metamorphic: Job dueness is symmetric around next_run time
    // If job is due 1 second after next_run, it should not be due 1 second before
    #[test]
    fn metamorphic_dueness_around_next_run() {
        let now = Utc::now();

        let mut job_past = Job::interval(
            "past".to_string(),
            "Past".to_string(),
            "Instructions".to_string(),
            60,
        );
        job_past.next_run = now - Duration::seconds(1); // 1 second in the past
        job_past.status = JobStatus::Pending;

        let mut job_future = Job::interval(
            "future".to_string(),
            "Future".to_string(),
            "Instructions".to_string(),
            60,
        );
        job_future.next_run = now + Duration::seconds(1); // 1 second in the future
        job_future.status = JobStatus::Pending;

        // Past should be due, future should not
        assert!(job_past.is_due(), "Job with past next_run should be due");
        assert!(
            !job_future.is_due(),
            "Job with future next_run should not be due"
        );
    }
}
