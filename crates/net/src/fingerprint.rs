//! Self-signed peer identity via SHA-256 certificate fingerprint.

use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Length of a SHA-256 digest in bytes.
const DIGEST_LEN: usize = 32;

/// SHA-256 digest of a peer's DER-encoded certificate.
///
/// Wire representation (used in logs, CLI, and the TOML database) is
/// `"sha256:<64 hex digits>"`. The prefix is mandatory so that if the
/// project ever adopts a second hash algorithm, old and new entries can
/// coexist in the same file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Fingerprint([u8; DIGEST_LEN]);

impl Fingerprint {
    /// Compute the fingerprint of a DER-encoded certificate.
    #[must_use]
    pub fn from_cert_der(der: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(der);
        Self(hasher.finalize().into())
    }

    /// Raw 32-byte digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Fingerprint {
    type Err = FingerprintParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some(hex) = s.strip_prefix("sha256:") else {
            return Err(FingerprintParseError::MissingPrefix);
        };
        if hex.len() != DIGEST_LEN * 2 {
            return Err(FingerprintParseError::WrongLength(hex.len()));
        }
        let mut out = [0u8; DIGEST_LEN];
        for (i, byte) in out.iter_mut().enumerate() {
            let pair = &hex[i * 2..i * 2 + 2];
            *byte = u8::from_str_radix(pair, 16).map_err(|_| FingerprintParseError::NotHex)?;
        }
        Ok(Self(out))
    }
}

impl TryFrom<String> for Fingerprint {
    type Error = FingerprintParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Fingerprint> for String {
    fn from(value: Fingerprint) -> Self {
        value.to_string()
    }
}

/// Errors from parsing a fingerprint string.
#[derive(Debug, Error)]
pub enum FingerprintParseError {
    /// The string did not start with `"sha256:"`.
    #[error("fingerprint must start with 'sha256:'")]
    MissingPrefix,
    /// The hex body was not exactly 64 characters.
    #[error("fingerprint hex body must be 64 chars, got {0}")]
    WrongLength(usize),
    /// The hex body contained non-hex characters.
    #[error("fingerprint contained non-hex characters")]
    NotHex,
}

/// One entry in the fingerprint database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEntry {
    /// Human-readable name (e.g. `"laptop"`).
    pub name: String,
    /// The peer's TLS certificate fingerprint.
    pub fingerprint: Fingerprint,
    /// When this entry was first added (UTC).
    pub added: DateTime<Utc>,
}

/// A list of trusted peer fingerprints, backed by a TOML file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintDb {
    /// Trusted peers, indexed by fingerprint at lookup time.
    #[serde(default, rename = "peer")]
    peers: Vec<PeerEntry>,
}

impl FingerprintDb {
    /// Build an empty database.
    #[must_use]
    pub fn new() -> Self {
        Self { peers: Vec::new() }
    }

    /// Load a database from disk. A missing file is treated as an empty DB,
    /// so a fresh install boots without an extra step.
    pub fn load(path: &Path) -> Result<Self, FingerprintDbError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let db: Self =
                    toml::from_str(&text).map_err(|e| FingerprintDbError::Parse(e.to_string()))?;
                Ok(db)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(err) => Err(FingerprintDbError::Io(err)),
        }
    }

    /// Persist the database to disk, overwriting any existing file.
    pub fn save(&self, path: &Path) -> Result<(), FingerprintDbError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| FingerprintDbError::Serialize(e.to_string()))?;
        fs::write(path, text)?;
        Ok(())
    }

    /// Look up a fingerprint. Returns the entry describing the peer if
    /// present, or `None` if the fingerprint is unknown.
    #[must_use]
    pub fn lookup(&self, fingerprint: &Fingerprint) -> Option<&PeerEntry> {
        self.peers.iter().find(|p| &p.fingerprint == fingerprint)
    }

    /// Add a new peer. If an entry with the same fingerprint already
    /// exists, it is replaced.
    pub fn add(&mut self, entry: PeerEntry) {
        self.peers.retain(|p| p.fingerprint != entry.fingerprint);
        self.peers.push(entry);
    }

    /// Remove the entry with the given name. Returns `true` if it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| p.name != name);
        self.peers.len() != before
    }

    /// Iterate over all peers.
    pub fn iter(&self) -> impl Iterator<Item = &PeerEntry> {
        self.peers.iter()
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Is the DB empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

/// Errors from loading or saving the fingerprint DB.
#[derive(Debug, Error)]
pub enum FingerprintDbError {
    /// Underlying I/O error (read or write).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// TOML parse failure while loading.
    #[error("failed to parse fingerprint DB: {0}")]
    Parse(String),

    /// TOML serialization failure while saving.
    #[error("failed to serialize fingerprint DB: {0}")]
    Serialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_roundtrips_through_string() {
        let fp = Fingerprint::from_cert_der(b"hello");
        let s = fp.to_string();
        assert!(s.starts_with("sha256:"));
        let parsed: Fingerprint = s.parse().unwrap();
        assert_eq!(fp, parsed);
    }

    #[test]
    fn fingerprint_rejects_bad_strings() {
        assert!("nope".parse::<Fingerprint>().is_err());
        assert!("sha256:tooshort".parse::<Fingerprint>().is_err());
        assert!(
            "sha256:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
                .parse::<Fingerprint>()
                .is_err()
        );
    }

    #[test]
    fn db_roundtrips_through_toml() {
        let mut db = FingerprintDb::new();
        db.add(PeerEntry {
            name: "laptop".into(),
            fingerprint: Fingerprint::from_cert_der(b"cert-a"),
            added: Utc::now(),
        });
        db.add(PeerEntry {
            name: "desktop".into(),
            fingerprint: Fingerprint::from_cert_der(b"cert-b"),
            added: Utc::now(),
        });

        let text = toml::to_string_pretty(&db).unwrap();
        let back: FingerprintDb = toml::from_str(&text).unwrap();
        assert_eq!(back.len(), 2);
        assert!(back
            .lookup(&Fingerprint::from_cert_der(b"cert-a"))
            .is_some());
    }

    #[test]
    fn db_load_missing_file_is_empty() {
        let tmp = std::env::temp_dir().join("hop-nonexistent.toml");
        let _ = std::fs::remove_file(&tmp);
        let db = FingerprintDb::load(&tmp).unwrap();
        assert!(db.is_empty());
    }

    #[test]
    fn add_same_fingerprint_replaces_entry() {
        let mut db = FingerprintDb::new();
        let fp = Fingerprint::from_cert_der(b"same");
        db.add(PeerEntry {
            name: "old".into(),
            fingerprint: fp,
            added: Utc::now(),
        });
        db.add(PeerEntry {
            name: "new".into(),
            fingerprint: fp,
            added: Utc::now(),
        });
        assert_eq!(db.len(), 1);
        assert_eq!(db.lookup(&fp).unwrap().name, "new");
    }
}
