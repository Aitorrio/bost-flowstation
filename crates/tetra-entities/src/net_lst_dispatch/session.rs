//! Exclusive LST dispatch session lock (one browser operator at a time).

use std::time::{Duration, Instant};
use uuid::Uuid;

/// Heartbeat / claim TTL. Lost Wi-Fi must not hold the lock forever.
pub const SESSION_TTL: Duration = Duration::from_secs(20);
pub const HEARTBEAT_HINT_SECS: u64 = 8;

#[derive(Debug, Clone)]
pub struct SessionOwner {
    pub token: Uuid,
    pub claimed_at: Instant,
    pub last_seen: Instant,
    pub client_label: String,
}

#[derive(Debug, Default)]
pub struct SessionLock {
    owner: Option<SessionOwner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimResult {
    Ok { token: Uuid },
    Busy { holder: String },
}

impl SessionLock {
    pub fn claim(&mut self, client_label: String) -> ClaimResult {
        self.expire_if_stale();
        if let Some(ref o) = self.owner {
            return ClaimResult::Busy {
                holder: o.client_label.clone(),
            };
        }
        let token = Uuid::new_v4();
        let now = Instant::now();
        self.owner = Some(SessionOwner {
            token,
            claimed_at: now,
            last_seen: now,
            client_label,
        });
        ClaimResult::Ok { token }
    }

    pub fn heartbeat(&mut self, token: Uuid) -> bool {
        self.expire_if_stale();
        match self.owner.as_mut() {
            Some(o) if o.token == token => {
                o.last_seen = Instant::now();
                true
            }
            _ => false,
        }
    }

    pub fn release(&mut self, token: Uuid) -> bool {
        match &self.owner {
            Some(o) if o.token == token => {
                self.owner = None;
                true
            }
            _ => false,
        }
    }

    /// Force-clear (entity teardown / overload).
    pub fn force_clear(&mut self) {
        self.owner = None;
    }

    pub fn is_owner(&self, token: Uuid) -> bool {
        self.owner.as_ref().is_some_and(|o| o.token == token && o.last_seen.elapsed() < SESSION_TTL)
    }

    pub fn busy_holder(&mut self) -> Option<String> {
        self.expire_if_stale();
        self.owner.as_ref().map(|o| o.client_label.clone())
    }

    pub fn has_owner(&mut self) -> bool {
        self.expire_if_stale();
        self.owner.is_some()
    }

    fn expire_if_stale(&mut self) {
        if let Some(ref o) = self.owner
            && o.last_seen.elapsed() >= SESSION_TTL
        {
            tracing::info!("LST dispatch: session expired for {}", o.client_label);
            self.owner = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_claim_busy() {
        let mut lock = SessionLock::default();
        assert!(matches!(lock.claim("a".into()), ClaimResult::Ok { .. }));
        assert!(matches!(lock.claim("b".into()), ClaimResult::Busy { .. }));
    }

    #[test]
    fn release_allows_reclaim() {
        let mut lock = SessionLock::default();
        let ClaimResult::Ok { token } = lock.claim("a".into()) else {
            panic!("claim");
        };
        assert!(lock.release(token));
        assert!(matches!(lock.claim("b".into()), ClaimResult::Ok { .. }));
    }
}
