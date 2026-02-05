//! Stateful property testing for sync/recovery logic.
//!
//! Uses proptest-state-machine to exercise edge cases in firehose sync
//! and cache recovery logic. The state machine model tracks:
//!
//! - Sync state transitions (Disconnected -> Syncing -> Live)
//! - Firehose sequence number monotonicity
//! - Pending events queue behavior (bounded at 10k)
//! - Error recovery and cursor invalidation

use std::collections::VecDeque;
use std::sync::Arc;

use proptest::prelude::*;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use tokio::runtime::Runtime;

use winter_atproto::cache::{FirehoseCommit, FirehoseOp, RepoCache, SyncState};

/// Maximum pending events (must match cache.rs constant).
const MAX_PENDING_EVENTS: usize = 10_000;

/// Error types that can occur during firehose operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirehoseErrorType {
    /// Cursor is ahead of the firehose (invalid).
    FutureCursor,
    /// Consumer is too slow, cursor has been evicted.
    ConsumerTooSlow,
    /// Generic connection error.
    ConnectionError,
    /// Read timeout.
    Timeout,
}

/// Operations that can be performed on the sync system.
#[derive(Debug, Clone)]
pub enum SyncOperation {
    /// Connect to firehose, optionally with a cursor.
    Connect { with_cursor: bool },
    /// Disconnect from firehose.
    Disconnect,
    /// Receive a commit event with given sequence number.
    ReceiveCommit { seq: i64, ops_count: usize },
    /// Receive an error from firehose.
    ReceiveError { error_type: FirehoseErrorType },
    /// Complete a CAR fetch (transition from Syncing to Live).
    CompleteCarFetch,
    /// Replay pending events after CAR fetch.
    ReplayPendingEvents,
    /// Flood events to test queue overflow.
    FloodEvents { count: usize },
    /// Clear pending events (simulates restart).
    ClearPending,
}

/// Reference model for the sync system state.
#[derive(Clone, Debug, Default)]
pub struct SyncSystemModel {
    /// Current sync state.
    pub sync_state: SyncStateModel,
    /// Last seen firehose sequence number.
    pub firehose_seq: i64,
    /// Pending events during sync.
    pub pending_events: VecDeque<i64>,
    /// Whether connected to firehose.
    pub connected: bool,
    /// Whether cursor is valid (not invalidated by error).
    pub cursor_valid: bool,
}

/// Model of SyncState for the reference state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncStateModel {
    #[default]
    Disconnected,
    Syncing,
    Live,
}

impl From<SyncState> for SyncStateModel {
    fn from(state: SyncState) -> Self {
        match state {
            SyncState::Disconnected => SyncStateModel::Disconnected,
            SyncState::Syncing => SyncStateModel::Syncing,
            SyncState::Live => SyncStateModel::Live,
        }
    }
}

impl ReferenceStateMachine for SyncSystemModel {
    type State = Self;
    type Transition = SyncOperation;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(Self::default()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        // Generate transitions based on current state
        match state.sync_state {
            SyncStateModel::Disconnected => {
                // Can only connect when disconnected
                prop_oneof![
                    2 => any::<bool>().prop_map(|with_cursor| SyncOperation::Connect { with_cursor }),
                ]
                .boxed()
            }
            SyncStateModel::Syncing => {
                // During sync: receive commits, errors, or complete CAR fetch
                prop_oneof![
                    // Normal commit reception
                    3 => (1i64..10000i64, 0usize..5usize).prop_map(|(seq, ops_count)| {
                        SyncOperation::ReceiveCommit { seq, ops_count }
                    }),
                    // Error scenarios
                    1 => prop_oneof![
                        Just(SyncOperation::ReceiveError { error_type: FirehoseErrorType::FutureCursor }),
                        Just(SyncOperation::ReceiveError { error_type: FirehoseErrorType::ConsumerTooSlow }),
                        Just(SyncOperation::ReceiveError { error_type: FirehoseErrorType::ConnectionError }),
                        Just(SyncOperation::ReceiveError { error_type: FirehoseErrorType::Timeout }),
                    ],
                    // Complete sync
                    2 => Just(SyncOperation::CompleteCarFetch),
                    // Flood events (stress test)
                    1 => (100usize..500usize).prop_map(|count| SyncOperation::FloodEvents { count }),
                    // Disconnect
                    1 => Just(SyncOperation::Disconnect),
                    // Clear pending (simulates restart)
                    1 => Just(SyncOperation::ClearPending),
                ]
                .boxed()
            }
            SyncStateModel::Live => {
                // When live: receive commits, replay, disconnect, or flood
                prop_oneof![
                    // Normal commit reception (applied immediately when live)
                    4 => (1i64..10000i64, 0usize..5usize).prop_map(|(seq, ops_count)| {
                        SyncOperation::ReceiveCommit { seq, ops_count }
                    }),
                    // Replay events (should be no-op when live)
                    1 => Just(SyncOperation::ReplayPendingEvents),
                    // Disconnect
                    1 => Just(SyncOperation::Disconnect),
                    // Error scenarios (causes transition back to Syncing)
                    1 => prop_oneof![
                        Just(SyncOperation::ReceiveError { error_type: FirehoseErrorType::ConnectionError }),
                        Just(SyncOperation::ReceiveError { error_type: FirehoseErrorType::Timeout }),
                    ],
                ]
                .boxed()
            }
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            SyncOperation::Connect { with_cursor } => {
                state.connected = true;
                state.cursor_valid = *with_cursor && state.firehose_seq > 0;
                if state.sync_state == SyncStateModel::Disconnected {
                    state.sync_state = SyncStateModel::Syncing;
                }
            }
            SyncOperation::Disconnect => {
                state.connected = false;
                // When Live and disconnect, go back to Syncing
                if state.sync_state == SyncStateModel::Live {
                    state.sync_state = SyncStateModel::Syncing;
                }
            }
            SyncOperation::ReceiveCommit { seq, .. } => {
                // Only update seq if greater (monotonic)
                if *seq > state.firehose_seq {
                    state.firehose_seq = *seq;
                }

                // Queue or apply based on state
                match state.sync_state {
                    SyncStateModel::Disconnected | SyncStateModel::Syncing => {
                        // Queue the event
                        state.pending_events.push_back(*seq);
                        // Enforce max pending events (drop oldest)
                        while state.pending_events.len() > MAX_PENDING_EVENTS {
                            state.pending_events.pop_front();
                        }
                    }
                    SyncStateModel::Live => {
                        // Applied immediately, nothing to queue
                    }
                }
            }
            SyncOperation::ReceiveError { error_type } => {
                match error_type {
                    FirehoseErrorType::FutureCursor | FirehoseErrorType::ConsumerTooSlow => {
                        // Cursor is invalid, must reset
                        state.cursor_valid = false;
                        state.firehose_seq = 0;
                        state.connected = false;
                    }
                    FirehoseErrorType::ConnectionError | FirehoseErrorType::Timeout => {
                        // Disconnect but keep cursor valid
                        state.connected = false;
                        if state.sync_state == SyncStateModel::Live {
                            state.sync_state = SyncStateModel::Syncing;
                        }
                    }
                }
            }
            SyncOperation::CompleteCarFetch => {
                if state.sync_state == SyncStateModel::Syncing && state.connected {
                    state.sync_state = SyncStateModel::Live;
                }
            }
            SyncOperation::ReplayPendingEvents => {
                // Clear pending events (they've been replayed)
                if state.sync_state == SyncStateModel::Live {
                    state.pending_events.clear();
                }
            }
            SyncOperation::FloodEvents { count } => {
                // Queue many events - each event has seq = base + i
                let base_seq = state.firehose_seq;
                for i in 0..*count {
                    let seq = base_seq + 1 + i as i64;
                    state.pending_events.push_back(seq);
                }
                // Final seq is the last one we saw
                state.firehose_seq = base_seq + *count as i64;
                // Enforce max
                while state.pending_events.len() > MAX_PENDING_EVENTS {
                    state.pending_events.pop_front();
                }
            }
            SyncOperation::ClearPending => {
                state.pending_events.clear();
            }
        }
        state
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        match transition {
            SyncOperation::Connect { .. } => {
                // Can connect when disconnected or reconnecting
                !state.connected
            }
            SyncOperation::Disconnect => state.connected,
            SyncOperation::ReceiveCommit { .. } => state.connected,
            SyncOperation::ReceiveError { .. } => state.connected,
            SyncOperation::CompleteCarFetch => {
                state.sync_state == SyncStateModel::Syncing && state.connected
            }
            SyncOperation::ReplayPendingEvents => state.sync_state == SyncStateModel::Live,
            SyncOperation::FloodEvents { .. } => {
                state.sync_state == SyncStateModel::Syncing && state.connected
            }
            SyncOperation::ClearPending => true,
        }
    }
}

/// Test harness that wraps the real RepoCache with a tokio runtime.
pub struct SyncTestHarness {
    runtime: Runtime,
    cache: Arc<RepoCache>,
}

impl SyncTestHarness {
    fn new() -> Self {
        let runtime = Runtime::new().expect("Failed to create tokio runtime");
        let cache = RepoCache::new();
        Self { runtime, cache }
    }

    fn apply_operation(&self, op: &SyncOperation) {
        self.runtime.block_on(async {
            match op {
                SyncOperation::Connect { with_cursor } => {
                    // Simulate connection by setting state to Syncing
                    if self.cache.state() == SyncState::Disconnected {
                        self.cache.set_state(SyncState::Syncing);
                    }
                    if *with_cursor {
                        // Keep existing cursor
                    } else {
                        // Reset cursor for fresh sync
                        self.cache.reset_firehose_seq();
                    }
                }
                SyncOperation::Disconnect => {
                    // When Live and disconnect, go back to Syncing
                    if self.cache.state() == SyncState::Live {
                        self.cache.set_state(SyncState::Syncing);
                    }
                }
                SyncOperation::ReceiveCommit { seq, ops_count } => {
                    // Update sequence number
                    self.cache.update_firehose_seq(*seq);

                    // Create a commit with dummy operations
                    let ops: Vec<_> = (0..*ops_count)
                        .map(|i| FirehoseOp::CreateOrUpdate {
                            collection: "test.collection".to_string(),
                            rkey: format!("rkey{}", i),
                            cid: format!("cid{}", i),
                            record: vec![],
                        })
                        .collect();

                    let commit = FirehoseCommit {
                        seq: *seq,
                        rev: format!("rev{}", seq),
                        ops,
                    };

                    // Queue or apply based on state
                    match self.cache.state() {
                        SyncState::Disconnected | SyncState::Syncing => {
                            self.cache.queue_commit(commit).await;
                        }
                        SyncState::Live => {
                            // In real code, this would apply to cache
                            // Here we just track the seq update
                        }
                    }
                }
                SyncOperation::ReceiveError { error_type } => {
                    match error_type {
                        FirehoseErrorType::FutureCursor | FirehoseErrorType::ConsumerTooSlow => {
                            // Reset cursor
                            self.cache.reset_firehose_seq();
                            if self.cache.state() == SyncState::Live {
                                self.cache.set_state(SyncState::Syncing);
                            }
                        }
                        FirehoseErrorType::ConnectionError | FirehoseErrorType::Timeout => {
                            // Set back to Syncing if Live
                            if self.cache.state() == SyncState::Live {
                                self.cache.set_state(SyncState::Syncing);
                            }
                        }
                    }
                }
                SyncOperation::CompleteCarFetch => {
                    if self.cache.state() == SyncState::Syncing {
                        self.cache.set_state(SyncState::Live);
                    }
                }
                SyncOperation::ReplayPendingEvents => {
                    // Drain pending events
                    let _ = self.cache.drain_pending().await;
                }
                SyncOperation::FloodEvents { count } => {
                    // Queue many events
                    let base_seq = self.cache.firehose_seq();
                    for i in 0..*count {
                        let seq = base_seq + 1 + i as i64;
                        self.cache.update_firehose_seq(seq);
                        let commit = FirehoseCommit {
                            seq,
                            rev: format!("rev{}", seq),
                            ops: vec![],
                        };
                        self.cache.queue_commit(commit).await;
                    }
                }
                SyncOperation::ClearPending => {
                    self.cache.clear_pending().await;
                }
            }
        });
    }

    fn verify_invariants(&self, model: &SyncSystemModel) {
        self.runtime.block_on(async {
            // Invariant 1: State matches model
            let actual_state: SyncStateModel = self.cache.state().into();
            assert_eq!(
                actual_state, model.sync_state,
                "State mismatch: actual {:?} vs model {:?}",
                actual_state, model.sync_state
            );

            // Invariant 2: Sequence number matches model
            let actual_seq = self.cache.firehose_seq();
            assert_eq!(
                actual_seq, model.firehose_seq,
                "Sequence mismatch: actual {} vs model {}",
                actual_seq, model.firehose_seq
            );

            // Invariant 3: Pending events count is bounded
            let pending = self.cache.drain_pending().await;
            assert!(
                pending.len() <= MAX_PENDING_EVENTS,
                "Pending events {} exceeds max {}",
                pending.len(),
                MAX_PENDING_EVENTS
            );

            // Re-queue the events for future operations
            for commit in pending {
                self.cache.queue_commit(commit).await;
            }
        });
    }
}

impl StateMachineTest for SyncTestHarness {
    type SystemUnderTest = Self;
    type Reference = SyncSystemModel;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        Self::new()
    }

    fn apply(
        state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        state.apply_operation(&transition);
        state.verify_invariants(ref_state);
        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        state.verify_invariants(ref_state);
    }
}

// Run the state machine tests
prop_state_machine! {
    #![proptest_config(ProptestConfig {
        // Use fewer cases for CI, increase with PROPTEST_CASES env var
        cases: 100,
        max_shrink_iters: 10000,
        ..ProptestConfig::default()
    })]

    #[test]
    fn sync_state_machine_test(sequential 1..50 => SyncTestHarness);
}

// Additional targeted property tests

#[test]
fn test_firehose_seq_monotonic() {
    let cache = RepoCache::new();

    // Sequence should never decrease
    cache.update_firehose_seq(100);
    assert_eq!(cache.firehose_seq(), 100);

    cache.update_firehose_seq(50); // Lower value
    assert_eq!(cache.firehose_seq(), 100); // Should stay at 100

    cache.update_firehose_seq(200);
    assert_eq!(cache.firehose_seq(), 200);
}

#[test]
fn test_state_transitions_valid() {
    let cache = RepoCache::new();

    // Initial state
    assert_eq!(cache.state(), SyncState::Disconnected);

    // Disconnected -> Syncing (valid)
    cache.set_state(SyncState::Syncing);
    assert_eq!(cache.state(), SyncState::Syncing);

    // Syncing -> Live (valid)
    cache.set_state(SyncState::Live);
    assert_eq!(cache.state(), SyncState::Live);

    // Live -> Syncing (valid, on disconnect)
    cache.set_state(SyncState::Syncing);
    assert_eq!(cache.state(), SyncState::Syncing);
}

#[tokio::test]
async fn test_pending_events_bounded() {
    let cache = RepoCache::new();

    // Queue more than MAX_PENDING_EVENTS
    for i in 0..(MAX_PENDING_EVENTS + 500) {
        let commit = FirehoseCommit {
            seq: i as i64,
            rev: format!("rev{}", i),
            ops: vec![],
        };
        cache.queue_commit(commit).await;
    }

    // Drain and check count
    let pending = cache.drain_pending().await;
    assert_eq!(pending.len(), MAX_PENDING_EVENTS);

    // Oldest should have been dropped
    assert_eq!(pending[0].seq, 500); // First 500 were dropped
}

#[tokio::test]
async fn test_live_implies_empty_queue_after_drain() {
    let cache = RepoCache::new();

    // Queue some events during sync
    cache.set_state(SyncState::Syncing);
    for i in 0..10 {
        let commit = FirehoseCommit {
            seq: i,
            rev: format!("rev{}", i),
            ops: vec![],
        };
        cache.queue_commit(commit).await;
    }

    // Transition to Live and drain
    cache.set_state(SyncState::Live);
    let pending = cache.drain_pending().await;
    assert_eq!(pending.len(), 10);

    // Queue should be empty after drain
    let empty = cache.drain_pending().await;
    assert!(empty.is_empty());
}

#[tokio::test]
async fn test_clear_pending_discards_all() {
    let cache = RepoCache::new();

    // Queue events
    for i in 0..100 {
        let commit = FirehoseCommit {
            seq: i,
            rev: format!("rev{}", i),
            ops: vec![],
        };
        cache.queue_commit(commit).await;
    }

    // Clear pending
    cache.clear_pending().await;

    // Should be empty
    let pending = cache.drain_pending().await;
    assert!(pending.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn firehose_seq_never_decreases(
        updates in prop::collection::vec(0i64..100000i64, 1..100)
    ) {
        let cache = RepoCache::new();
        let mut max_seen = 0i64;

        for seq in updates {
            cache.update_firehose_seq(seq);
            if seq > max_seen {
                max_seen = seq;
            }
            prop_assert_eq!(cache.firehose_seq(), max_seen);
        }
    }

    #[test]
    fn queue_overflow_preserves_newest(
        event_count in (MAX_PENDING_EVENTS + 1)..(MAX_PENDING_EVENTS + 1000)
    ) {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let cache = RepoCache::new();

            for i in 0..event_count {
                let commit = FirehoseCommit {
                    seq: i as i64,
                    rev: format!("rev{}", i),
                    ops: vec![],
                };
                cache.queue_commit(commit).await;
            }

            let pending = cache.drain_pending().await;

            // Should have exactly MAX_PENDING_EVENTS
            prop_assert_eq!(pending.len(), MAX_PENDING_EVENTS);

            // Last event should be the most recent
            let last = pending.last().unwrap();
            prop_assert_eq!(last.seq, (event_count - 1) as i64);

            Ok(())
        })?;
    }
}
