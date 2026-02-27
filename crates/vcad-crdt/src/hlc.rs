//! Hybrid Logical Clock for causal ordering.
//!
//! Combines wall-clock time with a logical counter and replica ID to produce
//! globally unique, monotonically increasing timestamps.

use crate::ReplicaId;
use serde::{Deserialize, Serialize};

/// Hybrid logical clock timestamp.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HLC {
    /// Wall-clock milliseconds since UNIX epoch.
    pub wall_ms: u64,
    /// Logical counter for same-millisecond ordering.
    pub counter: u32,
    /// Replica that created this timestamp.
    pub replica: ReplicaId,
}

impl HLC {
    /// Create a new HLC for the given replica.
    pub fn new(replica: ReplicaId) -> Self {
        Self {
            wall_ms: 0,
            counter: 0,
            replica,
        }
    }

    fn now_ms() -> u64 {
        #[cfg(target_arch = "wasm32")]
        {
            js_sys::Date::now() as u64
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        }
    }

    /// Advance the clock for a local event.
    pub fn tick(&mut self) {
        let now = Self::now_ms();
        if now > self.wall_ms {
            self.wall_ms = now;
            self.counter = 0;
        } else {
            self.counter += 1;
        }
    }

    /// Update this clock after receiving a remote timestamp.
    pub fn receive(&mut self, remote: &HLC) {
        let now = Self::now_ms();
        if now > self.wall_ms && now > remote.wall_ms {
            self.wall_ms = now;
            self.counter = 0;
        } else if self.wall_ms == remote.wall_ms {
            self.counter = self.counter.max(remote.counter) + 1;
        } else if remote.wall_ms > self.wall_ms {
            self.wall_ms = remote.wall_ms;
            self.counter = remote.counter + 1;
        } else {
            // self.wall_ms > remote.wall_ms
            self.counter += 1;
        }
    }
}

impl PartialEq for HLC {
    fn eq(&self, other: &Self) -> bool {
        self.wall_ms == other.wall_ms
            && self.counter == other.counter
            && self.replica == other.replica
    }
}

impl Eq for HLC {}

impl PartialOrd for HLC {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HLC {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.wall_ms
            .cmp(&other.wall_ms)
            .then(self.counter.cmp(&other.counter))
            .then(self.replica.cmp(&other.replica))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tick_monotonic() {
        let mut hlc = HLC::new(ReplicaId(1));
        let t1 = {
            hlc.tick();
            hlc
        };
        let t2 = {
            hlc.tick();
            hlc
        };
        assert!(t2 > t1);
    }

    #[test]
    fn test_receive_advances() {
        let mut hlc1 = HLC::new(ReplicaId(1));
        let mut hlc2 = HLC::new(ReplicaId(2));

        hlc2.wall_ms = 999_999_999_999;
        hlc2.counter = 5;

        hlc1.receive(&hlc2);
        assert!(hlc1.wall_ms >= hlc2.wall_ms);
    }

    #[test]
    fn test_ordering_tiebreak_by_replica() {
        let a = HLC {
            wall_ms: 100,
            counter: 0,
            replica: ReplicaId(1),
        };
        let b = HLC {
            wall_ms: 100,
            counter: 0,
            replica: ReplicaId(2),
        };
        assert!(b > a);
    }
}
