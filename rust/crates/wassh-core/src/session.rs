//! Session lifecycle: status, reconnect policy and backoff.
//!
//! Session state rules shared by every transport, so the app can
//! share one implementation for SSH, Telnet, serial and local shells instead of the two
//! divergent copies that exist today.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Visible session state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Connecting,
    AwaitingHostKey,
    Authenticating,
    Connected,
    Reconnecting,
    Disconnected,
    Closed,
    Failed,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::AwaitingHostKey => "awaiting_host_key",
            Self::Authenticating => "authenticating",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Disconnected => "disconnected",
            Self::Closed => "closed",
            Self::Failed => "failed",
        }
    }

    /// True when a connection log should record a transition into this state.
    ///
    /// The intermediate states (`Connecting`, `Authenticating`, `AwaitingHostKey`) are noise
    /// for a log that answers "when was this host up?".
    pub fn is_loggable(self) -> bool {
        matches!(
            self,
            Self::Connected | Self::Reconnecting | Self::Disconnected | Self::Failed
        )
    }

    /// True while the UI should show the amber "in progress" indicator.
    pub fn is_in_progress(self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::AwaitingHostKey | Self::Authenticating | Self::Reconnecting
        )
    }

    /// True when the user can retry without losing the tab.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Disconnected | Self::Failed | Self::Reconnecting)
    }
}

/// One status change a session went through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionEvent {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// The state the session entered.
    pub status: SessionStatus,
    /// Reason attached to the change, when the transport gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// When to reconnect automatically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconnectMode {
    /// Never retry.
    None,
    /// Retry while the application window has focus. This is the default for new hosts.
    #[default]
    OnFocus,
    /// Always retry, capped by the backoff ceiling.
    Always,
}

impl ReconnectMode {
    pub fn allows_retry(self, app_focused: bool) -> bool {
        match self {
            Self::None => false,
            Self::OnFocus => app_focused,
            Self::Always => true,
        }
    }
}

/// Delay before the given attempt, where attempt 1 waits one second.
///
/// `min(1000 * 2^(attempt-1), 600_000)` milliseconds.
/// used (`RECONNECT_INITIAL_BACKOFF_MS` / `RECONNECT_MAX_BACKOFF_MS`).
pub fn backoff_delay(attempt: u32) -> Duration {
    const INITIAL_MS: u64 = 1_000;
    const MAX_MS: u64 = 600_000;

    let exponent = attempt.saturating_sub(1).min(20);
    let millis = INITIAL_MS.saturating_mul(1u64 << exponent).min(MAX_MS);
    Duration::from_millis(millis)
}

/// Banner text shown while waiting to retry, e.g. `Attempt 3 (in 4s)`.
pub fn reconnect_label(attempt: u32) -> String {
    let millis = backoff_delay(attempt).as_millis() as u64;
    let seconds = millis.div_ceil(1_000).max(1);
    format!("Attempt {attempt} (in {seconds}s)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_at_one_second_and_doubles() {
        assert_eq!(backoff_delay(1), Duration::from_millis(1_000));
        assert_eq!(backoff_delay(2), Duration::from_millis(2_000));
        assert_eq!(backoff_delay(3), Duration::from_millis(4_000));
        assert_eq!(backoff_delay(10), Duration::from_millis(512_000));
    }

    #[test]
    fn backoff_is_capped_at_ten_minutes() {
        for attempt in 11..40 {
            assert_eq!(backoff_delay(attempt), Duration::from_millis(600_000));
        }
        assert_eq!(backoff_delay(u32::MAX), Duration::from_millis(600_000));
    }

    #[test]
    fn label_matches_the_electron_wording() {
        assert_eq!(reconnect_label(1), "Attempt 1 (in 1s)");
        assert_eq!(reconnect_label(2), "Attempt 2 (in 2s)");
        assert_eq!(reconnect_label(11), "Attempt 11 (in 600s)");
    }

    #[test]
    fn retry_policy_respects_focus() {
        assert!(!ReconnectMode::None.allows_retry(true));
        assert!(ReconnectMode::OnFocus.allows_retry(true));
        assert!(!ReconnectMode::OnFocus.allows_retry(false));
        assert!(ReconnectMode::Always.allows_retry(false));
    }

    #[test]
    fn status_groups_match_the_ui() {
        assert!(SessionStatus::Connecting.is_in_progress());
        assert!(SessionStatus::AwaitingHostKey.is_in_progress());
        assert!(SessionStatus::Reconnecting.is_retryable());
        assert!(!SessionStatus::Connected.is_retryable());
        assert!(!SessionStatus::Closed.is_retryable());
        assert_eq!(SessionStatus::AwaitingHostKey.as_str(), "awaiting_host_key");
    }
}
