//! Host-key fingerprints in OpenSSH's `SHA256:<base64>` form.
//!
//! `known_hosts.json` stores a flat array of `{ host, port, fingerprint }` entries.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A verified host key, keyed by endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostKeyFingerprint {
    pub host: String,
    pub port: u16,
    /// `SHA256:<base64>` of the raw public-key blob.
    pub fingerprint: String,
}

impl HostKeyFingerprint {
    pub fn new(host: impl Into<String>, port: u16, fingerprint: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            fingerprint: fingerprint.into(),
        }
    }

    /// True when both fields describe the same endpoint.
    pub fn matches(&self, host: &str, port: u16) -> bool {
        self.host == host && self.port == port
    }
}

/// `SHA256:<base64-no-padding>` of the SHA-256 digest of the key blob.
pub fn fingerprint_sha256(key_blob: &[u8]) -> String {
    let digest = Sha256::digest(key_blob);
    format!("SHA256:{}", STANDARD_NO_PAD.encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_blob_matches_the_published_vector() {
        assert_eq!(
            fingerprint_sha256(b""),
            "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU"
        );
    }

    #[test]
    fn fingerprints_are_stable_and_unpadded() {
        let a = fingerprint_sha256(b"ssh-ed25519 AAAA");
        let b = fingerprint_sha256(b"ssh-ed25519 AAAA");
        assert_eq!(a, b);
        assert!(a.starts_with("SHA256:"));
        assert!(!a.contains('='));
        assert_ne!(a, fingerprint_sha256(b"ssh-ed25519 AAAB"));
    }

    #[test]
    fn endpoint_matching_compares_host_and_port() {
        let entry = HostKeyFingerprint::new("example.com", 22, "SHA256:x");
        assert!(entry.matches("example.com", 22));
        assert!(!entry.matches("example.com", 2222));
        assert!(!entry.matches("other.example.com", 22));
    }

    #[test]
    fn entries_serialise_with_their_endpoint_and_fingerprint() {
        let entry = HostKeyFingerprint::new("h", 22, "SHA256:x");
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"host\":\"h\""));
        assert!(json.contains("\"fingerprint\":\"SHA256:x\""));
    }
}
