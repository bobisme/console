//! Deterministic cart-to-host platform events.
//!
//! Carts can publish score state and request leaderboard UI, but they never
//! receive host state or call a vendor SDK directly. This keeps platform
//! failures and asynchronous UI outside the simulation contract.

use serde::Serialize;

/// Largest score accepted by the host-neutral platform surface: JavaScript's
/// exact-integer ceiling. A selected adapter or game service may impose a
/// narrower configured limit without changing deterministic core semantics.
pub const MAX_SCORE: u64 = 9_007_199_254_740_991;

/// Core events retained between host drains. Ordinary hosts drain after
/// `_init`, each frame, and each diagnostic eval; this bound protects hosts
/// that never inspect the stream from an accidental top-level loop.
pub const MAX_PLATFORM_EVENTS: usize = 256;

/// One ordered, host-neutral platform request made by cart Lua.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlatformEvent {
    ScoreUpdate { score: u64 },
    ScoreSubmit { score: u64 },
    LeaderboardShow,
}

impl PlatformEvent {
    /// Stable compact code used by the web C ABI.
    pub const fn abi_kind(&self) -> u32 {
        match self {
            PlatformEvent::ScoreUpdate { .. } => 1,
            PlatformEvent::ScoreSubmit { .. } => 2,
            PlatformEvent::LeaderboardShow => 3,
        }
    }

    /// Score payload for score events. Leaderboard requests have no payload.
    pub const fn score(&self) -> Option<u64> {
        match self {
            PlatformEvent::ScoreUpdate { score } | PlatformEvent::ScoreSubmit { score } => {
                Some(*score)
            }
            PlatformEvent::LeaderboardShow => None,
        }
    }
}

/// A bounded drain from the core event queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformEventFrame {
    pub capacity: usize,
    pub dropped: u32,
    pub events: Vec<PlatformEvent>,
}

#[derive(Debug, Default)]
pub(crate) struct PlatformState {
    current_score: u64,
    score_started: bool,
    score_submitted: bool,
    events: Vec<PlatformEvent>,
    dropped: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PlatformCheckpoint {
    current_score: u64,
    score_started: bool,
    score_submitted: bool,
    events_len: usize,
    dropped: u32,
}

impl PlatformState {
    pub fn checkpoint(&self) -> PlatformCheckpoint {
        PlatformCheckpoint {
            current_score: self.current_score,
            score_started: self.score_started,
            score_submitted: self.score_submitted,
            events_len: self.events.len(),
            dropped: self.dropped,
        }
    }

    pub fn rollback(&mut self, checkpoint: PlatformCheckpoint) {
        self.current_score = checkpoint.current_score;
        self.score_started = checkpoint.score_started;
        self.score_submitted = checkpoint.score_submitted;
        self.events.truncate(checkpoint.events_len);
        self.dropped = checkpoint.dropped;
    }

    fn push(&mut self, event: PlatformEvent) {
        if self.events.len() >= MAX_PLATFORM_EVENTS {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            self.events.push(event);
        }
    }

    /// Publish the score currently visible to the player. Identical updates
    /// within one unfinished result are suppressed. An update after submit
    /// starts a new result, including when its score equals the old result.
    pub fn update_score(&mut self, score: u64) {
        if self.score_started && !self.score_submitted && self.current_score == score {
            return;
        }
        self.current_score = score;
        self.score_started = true;
        self.score_submitted = false;
        self.push(PlatformEvent::ScoreUpdate { score });
    }

    /// Submit the current result exactly once. Before the first explicit
    /// update the deterministic current score is zero.
    pub fn submit_score(&mut self) {
        if self.score_submitted {
            return;
        }
        self.score_started = true;
        self.score_submitted = true;
        self.push(PlatformEvent::ScoreSubmit {
            score: self.current_score,
        });
    }

    /// Every call is an explicit UI request. Unlike score publication, these
    /// are not coalesced because a later call can represent a later user
    /// gesture after the host has closed its UI.
    pub fn show_leaderboard(&mut self) {
        self.push(PlatformEvent::LeaderboardShow);
    }

    pub fn take_events(&mut self) -> PlatformEventFrame {
        PlatformEventFrame {
            capacity: MAX_PLATFORM_EVENTS,
            dropped: std::mem::take(&mut self.dropped),
            events: std::mem::take(&mut self.events),
        }
    }
}
