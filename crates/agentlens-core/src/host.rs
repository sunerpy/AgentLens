//! 主机标识与注册表：host_id = SHA-256(machine-id) 截断 16 hex（Linux 读 `/etc/machine-id`，
//! fallback `/var/lib/dbus/machine-id`；Windows 读 `MachineGuid`），以及 `hosts` 表 CRUD
//! 与 machine_id_hash 唯一约束（防同机双计）。
//!
//! # Hashing contract (wire-visible, must be reproduced byte for byte)
//!
//! The remote collector computes the same identity on the remote machine and returns
//! `machine_id_hash` in its NDJSON meta line, so every step below is a wire contract. Any
//! divergence silently splits one physical machine into two hosts and double counts its usage.
//!
//! 1. Read the machine-id source as bytes and reject it when it exceeds
//!    [`MACHINE_ID_MAX_BYTES`].
//! 2. Decode as UTF-8, then **trim leading and trailing whitespace** (`str::trim`). A file with a
//!    trailing newline therefore produces the same identity as one without. An empty or
//!    whitespace-only source counts as absent and the discovery chain moves on.
//! 3. Hash the trimmed UTF-8 **string bytes** with SHA-256 — never the raw file bytes.
//! 4. `machine_id_hash` = the full lowercase 64-hex digest ([`MACHINE_ID_HASH_HEX_LENGTH`]).
//! 5. `host_id` = the first 16 hex characters of that digest ([`HOST_ID_HEX_LENGTH`]), which is the
//!    archive key inside `UNIQUE(host_id, source, message_id)`.
//!
//! The reference shell equivalent for the usual single-line file is:
//!
//! ```text
//! printf '%s' "$(cat /etc/machine-id)" | sha256sum   # -> machine_id_hash; first 16 chars -> host_id
//! ```
//!
//! A non-hex but non-empty machine id is still hashable and accepted as-is; the value is treated as
//! an opaque stable string, not as a number.

use std::fs;
use std::io;
use std::path::Path;
use std::str;

use rusqlite::{params, Connection, OptionalExtension as _, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// Number of hex characters kept from the SHA-256 digest to form a `host_id`.
pub const HOST_ID_HEX_LENGTH: usize = 16;

/// Number of hex characters in a full `machine_id_hash`.
pub const MACHINE_ID_HASH_HEX_LENGTH: usize = 64;

/// Largest accepted machine-id source file, in bytes.
///
/// A real machine id is 32 hex characters plus a newline. The generous 4 KiB ceiling keeps a
/// corrupt or unrelated file (a log, a truncated image) from being hashed into a bogus identity
/// while never rejecting a legitimate source.
pub const MACHINE_ID_MAX_BYTES: usize = 4096;

/// Linux machine-id discovery chain, in priority order.
pub const LINUX_MACHINE_ID_SOURCES: [&str; 2] = ["/etc/machine-id", "/var/lib/dbus/machine-id"];

/// Result type returned by host identity and registry operations.
pub type Result<T> = std::result::Result<T, HostError>;

/// Errors returned while deriving a machine identity or maintaining the `hosts` registry.
#[derive(Debug, Error)]
pub enum HostError {
    /// No machine-id source in the discovery chain produced a usable value.
    #[error(
        "cannot read a stable machine id from any known source: {attempted}; on Linux run `systemd-machine-id-setup` or write a fresh 32-hex id to /etc/machine-id and restart AgentLens, otherwise register this host with an explicit machine id"
    )]
    MachineIdUnavailable {
        /// Every attempted source with the reason it was rejected.
        attempted: String,
    },
    /// A machine id was supplied directly but is empty or whitespace only.
    #[error(
        "machine id is empty after trimming; supply the non-empty contents of /etc/machine-id"
    )]
    MachineIdBlank,
    /// A `machine_id_hash` was supplied directly but is not a 64-character lowercase hex digest.
    #[error(
        "machine id hash {value:?} is not a {MACHINE_ID_HASH_HEX_LENGTH}-character lowercase hex SHA-256 digest"
    )]
    InvalidMachineIdHash {
        /// Rejected value.
        value: String,
    },
    /// Reading the Windows `MachineGuid` registry value failed.
    #[cfg(windows)]
    #[error(
        "cannot read HKLM\\SOFTWARE\\Microsoft\\Cryptography\\MachineGuid: {detail}; run `reg query HKLM\\SOFTWARE\\Microsoft\\Cryptography /v MachineGuid` in a console to confirm the value exists"
    )]
    MachineGuidUnavailable {
        /// Underlying failure detail.
        detail: String,
    },
    /// Another host already claims this machine, which would double count its usage.
    #[error(
        "machine id hash {machine_id_hash} is already registered as host {existing_host_id}; 与主机 {existing_display_name} 重复，同一台机器不能重复添加（否则用量会被双计）"
    )]
    DuplicateMachine {
        /// Machine hash shared by both registrations.
        machine_id_hash: String,
        /// Existing `host_id` holding that machine hash.
        existing_host_id: String,
        /// Existing host display name, echoed for the UI message.
        existing_display_name: String,
    },
    /// The `host_id` is already present with a different machine hash.
    #[error("host {host_id} is already registered as {display_name:?}")]
    HostAlreadyExists {
        /// Conflicting host identifier.
        host_id: String,
        /// Display name of the stored host.
        display_name: String,
    },
    /// The requested host is not in the registry.
    #[error("host {host_id} is not registered")]
    HostNotFound {
        /// Requested host identifier.
        host_id: String,
    },
    /// A stored or supplied `kind` is outside the `local` / `ssh` contract.
    #[error("host kind {value:?} is invalid; expected \"local\" or \"ssh\"")]
    InvalidHostKind {
        /// Rejected encoding.
        value: String,
    },
    /// A `local` host carries an SSH target, which the scheduler would misroute.
    #[error("host {host_id} is kind \"local\" but carries ssh_target {ssh_target:?}; clear the target or register it as kind \"ssh\"")]
    SshTargetOnLocalHost {
        /// Offending host identifier.
        host_id: String,
        /// Target that must be removed.
        ssh_target: String,
    },
    /// An `ssh` host has no target to connect to.
    #[error("host {host_id} is kind \"ssh\" but has no ssh_target; supply a target such as user@example")]
    MissingSshTarget {
        /// Offending host identifier.
        host_id: String,
    },
    /// The display name is empty or whitespace only.
    #[error("host display_name is empty; supply a name for the host list")]
    BlankDisplayName,
    /// The registry statement failed inside SQLite.
    #[error("host registry database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Stable identity of one physical machine.
///
/// Both fields are derived together so a caller can never pair a `host_id` with an unrelated
/// `machine_id_hash`; see the [module contract](self) for the exact derivation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineIdentity {
    machine_id_hash: String,
    host_id: String,
}

impl MachineIdentity {
    /// Derives an identity from a raw machine-id string.
    ///
    /// The value is trimmed before hashing, so a trailing newline does not change the result.
    pub fn from_machine_id(machine_id: &str) -> Result<Self> {
        let trimmed = machine_id.trim();
        if trimmed.is_empty() {
            return Err(HostError::MachineIdBlank);
        }

        let mut hasher = Sha256::new();
        hasher.update(trimmed.as_bytes());
        let machine_id_hash = hex::encode(hasher.finalize());
        let host_id = machine_id_hash[..HOST_ID_HEX_LENGTH].to_owned();

        Ok(Self {
            machine_id_hash,
            host_id,
        })
    }

    /// Rebuilds an identity from a `machine_id_hash` reported by a remote collector.
    ///
    /// The remote machine id itself never crosses the wire; only its digest does.
    pub fn from_machine_id_hash(machine_id_hash: &str) -> Result<Self> {
        let is_lowercase_hex = machine_id_hash.len() == MACHINE_ID_HASH_HEX_LENGTH
            && machine_id_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_lowercase_hex {
            return Err(HostError::InvalidMachineIdHash {
                value: machine_id_hash.to_owned(),
            });
        }

        Ok(Self {
            host_id: machine_id_hash[..HOST_ID_HEX_LENGTH].to_owned(),
            machine_id_hash: machine_id_hash.to_owned(),
        })
    }

    /// Returns the full lowercase 64-hex SHA-256 digest stored in `hosts.machine_id_hash`.
    pub fn machine_id_hash(&self) -> &str {
        &self.machine_id_hash
    }

    /// Returns the 16-hex archive key stored in `hosts.host_id`.
    pub fn host_id(&self) -> &str {
        &self.host_id
    }
}

/// Reads the first usable machine id from an injected discovery chain.
///
/// Sources are tried in order. A missing, unreadable, oversized, non-UTF-8, empty, or
/// whitespace-only source is skipped so a partially provisioned machine still resolves through the
/// fallback. When every source fails, the returned [`HostError::MachineIdUnavailable`] lists each
/// path with its rejection reason plus remediation.
pub fn machine_id_from_sources(sources: &[&Path]) -> Result<String> {
    let mut attempted = Vec::with_capacity(sources.len());

    for source in sources {
        match read_machine_id_source(source) {
            Ok(machine_id) => return Ok(machine_id),
            Err(reason) => attempted.push(format!("{} ({reason})", source.display())),
        }
    }

    Err(HostError::MachineIdUnavailable {
        attempted: attempted.join(", "),
    })
}

/// Reads this machine's id through the production discovery chain.
#[cfg(not(windows))]
pub fn local_machine_id() -> Result<String> {
    let sources: Vec<&Path> = LINUX_MACHINE_ID_SOURCES.iter().map(Path::new).collect();
    machine_id_from_sources(&sources)
}

/// Reads this machine's id from the Windows registry.
#[cfg(windows)]
pub fn local_machine_id() -> Result<String> {
    windows_machine_guid()
}

/// Derives this machine's stable identity through the production discovery chain.
pub fn local_machine_identity() -> Result<MachineIdentity> {
    MachineIdentity::from_machine_id(&local_machine_id()?)
}

/// Reads `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` through the bundled `reg` tool.
///
/// The value is read by shelling out to `reg query` rather than adding a `winreg` dependency: this
/// workspace is built and tested on Linux, where a Windows-only crate can be neither compiled nor
/// exercised, so the platform branch stays dependency-free and reviewable. The parsed GUID feeds
/// the same trim-then-hash contract as the Linux chain.
#[cfg(windows)]
fn windows_machine_guid() -> Result<String> {
    use std::process::Command;

    const REGISTRY_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Cryptography";
    const REGISTRY_VALUE: &str = "MachineGuid";

    let output = Command::new("reg")
        .args(["query", REGISTRY_KEY, "/v", REGISTRY_VALUE])
        .output()
        .map_err(|error| HostError::MachineGuidUnavailable {
            detail: format!("cannot run `reg query`: {error}"),
        })?;
    if !output.status.success() {
        return Err(HostError::MachineGuidUnavailable {
            detail: format!(
                "`reg query` exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let guid = stdout
        .lines()
        .find(|line| line.contains(REGISTRY_VALUE))
        .and_then(|line| line.split_whitespace().next_back())
        .map(str::trim)
        .filter(|guid| !guid.is_empty())
        .ok_or_else(|| HostError::MachineGuidUnavailable {
            detail: "`reg query` output did not contain a MachineGuid value".to_owned(),
        })?;

    Ok(guid.to_owned())
}

fn read_machine_id_source(path: &Path) -> std::result::Result<String, String> {
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            "not found".to_owned()
        } else {
            format!("unreadable: {error}")
        }
    })?;
    if bytes.len() > MACHINE_ID_MAX_BYTES {
        return Err(format!(
            "too long: {} bytes exceeds the {MACHINE_ID_MAX_BYTES} byte limit",
            bytes.len()
        ));
    }

    let text = str::from_utf8(&bytes).map_err(|error| format!("not valid UTF-8: {error}"))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty".to_owned());
    }

    Ok(trimmed.to_owned())
}

/// Collection mode of a registered host.
///
/// The string encodings are the exact text stored in `hosts.kind` and mirror the lowercase serde
/// style used by [`crate::archive::Origin`] and [`crate::archive::CostSource`], so todo 13 can
/// export all of them to TypeScript with one convention.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    /// Scanned in-process on this machine.
    Local,
    /// Scanned on a remote machine over SSH.
    Ssh,
}

impl HostKind {
    /// Returns the exact text stored in `hosts.kind`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
        }
    }

    /// Decodes the text stored in `hosts.kind`.
    pub fn from_encoded(value: &str) -> Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "ssh" => Ok(Self::Ssh),
            other => Err(HostError::InvalidHostKind {
                value: other.to_owned(),
            }),
        }
    }
}

/// One row of the `hosts` registry.
///
/// `host_id` and `machine_id_hash` are private because they are derived together by
/// [`MachineIdentity`]; the mutable fields are the user-editable ones the UI exposes.
/// `display_name` is free-form user input (an SSH alias is display only) and is always bound as a
/// SQL parameter, never interpolated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRecord {
    host_id: String,
    machine_id_hash: String,
    /// Name shown in the host list.
    pub display_name: String,
    /// Collection mode.
    pub kind: HostKind,
    /// SSH destination such as `user@example`; required for [`HostKind::Ssh`] and forbidden for
    /// [`HostKind::Local`].
    pub ssh_target: Option<String>,
    /// Optional override of the remote source data directory; `None` means "use the default".
    pub remote_data_dir: Option<String>,
    /// UTC epoch milliseconds of the last successful collection, or `None` before the first one.
    pub last_success_utc: Option<i64>,
}

impl HostRecord {
    /// Builds a local host that is collected in-process.
    pub fn local(display_name: impl Into<String>, identity: &MachineIdentity) -> Self {
        Self {
            host_id: identity.host_id().to_owned(),
            machine_id_hash: identity.machine_id_hash().to_owned(),
            display_name: display_name.into(),
            kind: HostKind::Local,
            ssh_target: None,
            remote_data_dir: None,
            last_success_utc: None,
        }
    }

    /// Builds an SSH host collected on a remote machine.
    pub fn ssh(
        display_name: impl Into<String>,
        ssh_target: impl Into<String>,
        identity: &MachineIdentity,
    ) -> Self {
        Self {
            host_id: identity.host_id().to_owned(),
            machine_id_hash: identity.machine_id_hash().to_owned(),
            display_name: display_name.into(),
            kind: HostKind::Ssh,
            ssh_target: Some(ssh_target.into()),
            remote_data_dir: None,
            last_success_utc: None,
        }
    }

    /// Overrides the remote source data directory.
    #[must_use]
    pub fn with_remote_data_dir(mut self, remote_data_dir: Option<String>) -> Self {
        self.remote_data_dir = remote_data_dir;
        self
    }

    /// Returns the 16-hex archive key.
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// Returns the full 64-hex machine digest that guards against double counting.
    pub fn machine_id_hash(&self) -> &str {
        &self.machine_id_hash
    }

    fn validate(&self) -> Result<()> {
        if self.display_name.trim().is_empty() {
            return Err(HostError::BlankDisplayName);
        }

        match (self.kind, self.ssh_target.as_deref()) {
            (HostKind::Local, Some(ssh_target)) => Err(HostError::SshTargetOnLocalHost {
                host_id: self.host_id.clone(),
                ssh_target: ssh_target.to_owned(),
            }),
            (HostKind::Ssh, None) => Err(HostError::MissingSshTarget {
                host_id: self.host_id.clone(),
            }),
            (HostKind::Ssh, Some(ssh_target)) if ssh_target.trim().is_empty() => {
                Err(HostError::MissingSshTarget {
                    host_id: self.host_id.clone(),
                })
            }
            _ => Ok(()),
        }
    }
}

/// CRUD access to the archive's `hosts` table.
///
/// The table itself is created by the archive v1 migration; this type only reads and writes rows.
/// Construct it from [`crate::archive::Archive::connection`].
pub struct HostRegistry<'connection> {
    connection: &'connection Connection,
}

const SELECT_HOST_COLUMNS: &str = "SELECT host_id, display_name, kind, ssh_target, \
     remote_data_dir, last_success_utc, machine_id_hash FROM hosts";

impl<'connection> HostRegistry<'connection> {
    /// Wraps an open archive connection.
    pub fn new(connection: &'connection Connection) -> Self {
        Self { connection }
    }

    /// Registers a new host.
    ///
    /// Rejects a machine that is already registered with [`HostError::DuplicateMachine`], whose
    /// message names the existing host so the UI can say which entry it collides with.
    pub fn insert(&self, host: &HostRecord) -> Result<()> {
        host.validate()?;

        let outcome = self.connection.execute(
            "INSERT INTO hosts (
                host_id, display_name, kind, ssh_target,
                remote_data_dir, last_success_utc, machine_id_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                host.host_id,
                host.display_name,
                host.kind.as_str(),
                host.ssh_target,
                host.remote_data_dir,
                host.last_success_utc,
                host.machine_id_hash,
            ],
        );

        match outcome {
            Ok(_) => Ok(()),
            Err(error) if is_unique_violation(&error) => Err(self.resolve_conflict(host, error)?),
            Err(error) => Err(HostError::Sqlite(error)),
        }
    }

    /// Reads one host by `host_id`.
    pub fn get(&self, host_id: &str) -> Result<Option<HostRecord>> {
        let mut statement = self
            .connection
            .prepare(&format!("{SELECT_HOST_COLUMNS} WHERE host_id = ?1"))?;
        Ok(statement
            .query_row(params![host_id], map_host_row)
            .optional()?)
    }

    /// Reads one host by its full machine digest.
    pub fn find_by_machine_id_hash(&self, machine_id_hash: &str) -> Result<Option<HostRecord>> {
        let mut statement = self
            .connection
            .prepare(&format!("{SELECT_HOST_COLUMNS} WHERE machine_id_hash = ?1"))?;
        Ok(statement
            .query_row(params![machine_id_hash], map_host_row)
            .optional()?)
    }

    /// Lists every registered host ordered by display name.
    pub fn list(&self) -> Result<Vec<HostRecord>> {
        let mut statement = self.connection.prepare(&format!(
            "{SELECT_HOST_COLUMNS} ORDER BY display_name, host_id"
        ))?;
        let hosts = statement
            .query_map([], map_host_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hosts)
    }

    /// Updates the user-editable fields of an existing host.
    ///
    /// `machine_id_hash` is immutable (identity never changes for a row) and `last_success_utc` is
    /// owned by [`Self::update_last_success`], so neither appears in the statement.
    pub fn update(&self, host: &HostRecord) -> Result<()> {
        host.validate()?;

        let affected = self.connection.execute(
            "UPDATE hosts
             SET display_name = ?2, kind = ?3, ssh_target = ?4, remote_data_dir = ?5
             WHERE host_id = ?1",
            params![
                host.host_id,
                host.display_name,
                host.kind.as_str(),
                host.ssh_target,
                host.remote_data_dir,
            ],
        )?;
        if affected == 0 {
            return Err(HostError::HostNotFound {
                host_id: host.host_id.clone(),
            });
        }
        Ok(())
    }

    /// Records a successful collection at `at_utc_ms` (UTC epoch milliseconds).
    ///
    /// Repeated calls overwrite the column in place, so no duplicate row can appear and the newest
    /// timestamp always wins.
    pub fn update_last_success(&self, host_id: &str, at_utc_ms: i64) -> Result<()> {
        let affected = self.connection.execute(
            "UPDATE hosts SET last_success_utc = ?2 WHERE host_id = ?1",
            params![host_id, at_utc_ms],
        )?;
        if affected == 0 {
            return Err(HostError::HostNotFound {
                host_id: host_id.to_owned(),
            });
        }
        Ok(())
    }

    /// Removes a host from the registry.
    pub fn delete(&self, host_id: &str) -> Result<()> {
        let affected = self
            .connection
            .execute("DELETE FROM hosts WHERE host_id = ?1", params![host_id])?;
        if affected == 0 {
            return Err(HostError::HostNotFound {
                host_id: host_id.to_owned(),
            });
        }
        Ok(())
    }

    fn resolve_conflict(&self, host: &HostRecord, original: rusqlite::Error) -> Result<HostError> {
        if let Some(existing) = self.find_by_machine_id_hash(&host.machine_id_hash)? {
            return Ok(HostError::DuplicateMachine {
                machine_id_hash: existing.machine_id_hash,
                existing_host_id: existing.host_id,
                existing_display_name: existing.display_name,
            });
        }
        if let Some(existing) = self.get(&host.host_id)? {
            return Ok(HostError::HostAlreadyExists {
                host_id: existing.host_id,
                display_name: existing.display_name,
            });
        }
        Ok(HostError::Sqlite(original))
    }
}

fn is_unique_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn map_host_row(row: &Row<'_>) -> rusqlite::Result<HostRecord> {
    let kind: String = row.get(2)?;
    let kind = HostKind::from_encoded(&kind).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;

    Ok(HostRecord {
        host_id: row.get(0)?,
        display_name: row.get(1)?,
        kind,
        ssh_target: row.get(3)?,
        remote_data_dir: row.get(4)?,
        last_success_utc: row.get(5)?,
        machine_id_hash: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crate::archive::Archive;

    const MACHINE_ID_A: &str = "3f8a2c1d4e5b6079a1b2c3d4e5f60718";
    const MACHINE_ID_B: &str = "aabbccddeeff00112233445566778899";
    const MACHINE_ID_C: &str = "0123456789abcdef0123456789abcdef";
    const HOSTILE_DISPLAY_NAME: &str = "Robert'); DROP TABLE hosts;--";

    fn identity(machine_id: &str) -> MachineIdentity {
        MachineIdentity::from_machine_id(machine_id).expect("derive identity from machine id")
    }

    #[test]
    fn host_duplicate_machine_id_registration_is_rejected_with_existing_display_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = Archive::open_in_data_dir(dir.path()).expect("open archive in tempdir");
        let registry = HostRegistry::new(archive.connection());

        let first = HostRecord::local("工作站-A", &identity(MACHINE_ID_A));
        registry
            .insert(&first)
            .expect("first registration succeeds");

        let duplicate = HostRecord::ssh("同机 SSH 别名", "user@127.0.0.1", &identity(MACHINE_ID_A));
        let error = registry
            .insert(&duplicate)
            .expect_err("second registration of the same machine must be rejected");
        let error_text = error.to_string();
        println!("duplicate_machine_error={error_text}");

        assert!(
            error_text.contains("工作站-A"),
            "error must name the existing host: {error_text}"
        );
        assert!(
            error_text.contains("与主机 工作站-A 重复"),
            "error must carry the planned Chinese remediation: {error_text}"
        );
        assert!(matches!(error, HostError::DuplicateMachine { .. }));
        assert_eq!(registry.list().expect("list hosts").len(), 1);
    }

    #[test]
    fn host_machine_id_fallback_chain_uses_second_source_then_reports_remediation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let primary = dir.path().join("etc-machine-id");
        let fallback = dir.path().join("dbus-machine-id");
        fs::write(&fallback, format!("{MACHINE_ID_B}\n")).expect("write fallback machine id");

        let from_fallback = machine_id_from_sources(&[primary.as_path(), fallback.as_path()])
            .expect("missing primary source must fall through to the fallback");
        assert_eq!(from_fallback, MACHINE_ID_B);

        fs::remove_file(&fallback).expect("remove fallback machine id");
        let error = machine_id_from_sources(&[primary.as_path(), fallback.as_path()])
            .expect_err("both sources missing must return Err");
        let error_text = error.to_string();
        println!("machine_id_unavailable_error={error_text}");

        assert!(error_text.contains(&primary.to_string_lossy().to_string()));
        assert!(error_text.contains(&fallback.to_string_lossy().to_string()));
        assert!(
            error_text.contains("systemd-machine-id-setup"),
            "remediation must tell the user how to create a machine id: {error_text}"
        );
        assert!(matches!(error, HostError::MachineIdUnavailable { .. }));
    }

    #[test]
    fn host_id_is_stable_across_calls_and_ignores_trailing_newline() {
        let bare = identity(MACHINE_ID_A);
        let again = identity(MACHINE_ID_A);
        let newline = identity(&format!("{MACHINE_ID_A}\n"));
        let padded = identity(&format!("  {MACHINE_ID_A}\r\n"));

        assert_eq!(bare.host_id(), again.host_id());
        assert_eq!(bare.machine_id_hash(), again.machine_id_hash());
        assert_eq!(bare.host_id(), newline.host_id());
        assert_eq!(bare.host_id(), padded.host_id());
        assert_ne!(bare.host_id(), identity(MACHINE_ID_B).host_id());
        assert_eq!(
            bare.host_id(),
            &bare.machine_id_hash()[..HOST_ID_HEX_LENGTH],
            "host_id must be the machine_id_hash prefix"
        );
    }

    #[test]
    fn host_id_is_sixteen_lowercase_hex_characters() {
        for machine_id in [MACHINE_ID_A, MACHINE_ID_B, "not-hex-but-still-hashable"] {
            let derived = identity(machine_id);
            let host_id = derived.host_id();
            println!("machine_id={machine_id} host_id={host_id}");

            assert_eq!(host_id.len(), HOST_ID_HEX_LENGTH);
            assert!(
                host_id
                    .chars()
                    .all(|character| character.is_ascii_digit()
                        || ('a'..='f').contains(&character)),
                "host_id must be lowercase hex: {host_id}"
            );
            assert_eq!(derived.machine_id_hash().len(), MACHINE_ID_HASH_HEX_LENGTH);
        }
    }

    #[test]
    fn host_machine_id_rejects_empty_whitespace_and_overlong_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = dir.path().join("empty");
        let blank = dir.path().join("whitespace");
        let overlong = dir.path().join("overlong");
        let good = dir.path().join("good");
        fs::write(&empty, b"").expect("write empty source");
        fs::write(&blank, b"   \n\t\n").expect("write whitespace source");
        fs::write(&overlong, "a".repeat(MACHINE_ID_MAX_BYTES + 1)).expect("write overlong source");
        fs::write(&good, MACHINE_ID_B).expect("write good source");

        let resolved = machine_id_from_sources(&[
            empty.as_path(),
            blank.as_path(),
            overlong.as_path(),
            good.as_path(),
        ])
        .expect("malformed sources are skipped in favour of the next candidate");
        assert_eq!(resolved, MACHINE_ID_B);

        let error =
            machine_id_from_sources(&[empty.as_path(), blank.as_path(), overlong.as_path()])
                .expect_err("only malformed sources must return Err");
        let error_text = error.to_string();
        println!("malformed_machine_id_error={error_text}");
        assert!(error_text.contains("empty"));
        assert!(error_text.contains("too long"));

        assert!(matches!(
            MachineIdentity::from_machine_id("   \n"),
            Err(HostError::MachineIdBlank)
        ));
        identity("not-hex-but-still-hashable");
    }

    #[test]
    fn host_registry_crud_round_trip_and_last_success_updates_are_not_lost() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = Archive::open_in_data_dir(dir.path()).expect("open archive in tempdir");
        let registry = HostRegistry::new(archive.connection());

        let local = HostRecord::local("本机", &identity(MACHINE_ID_A));
        let remote = HostRecord::ssh("构建机", "deploy@build-01", &identity(MACHINE_ID_B))
            .with_remote_data_dir(Some("/srv/opencode".to_owned()));
        registry.insert(&local).expect("insert local host");
        registry.insert(&remote).expect("insert ssh host");

        let stored_local = registry
            .get(local.host_id())
            .expect("get local host")
            .expect("local host must exist");
        assert_eq!(stored_local, local);
        assert_eq!(stored_local.kind, HostKind::Local);
        assert_eq!(stored_local.ssh_target, None);
        assert_eq!(stored_local.remote_data_dir, None);
        assert_eq!(stored_local.last_success_utc, None);

        let stored_remote = registry
            .get(remote.host_id())
            .expect("get ssh host")
            .expect("ssh host must exist");
        assert_eq!(stored_remote.kind, HostKind::Ssh);
        assert_eq!(stored_remote.ssh_target.as_deref(), Some("deploy@build-01"));
        assert_eq!(
            stored_remote.remote_data_dir.as_deref(),
            Some("/srv/opencode")
        );

        let listed = registry.list().expect("list hosts");
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|host| host.host_id() == local.host_id()));
        assert!(listed.iter().any(|host| host.host_id() == remote.host_id()));

        let renamed = HostRecord::ssh("构建机（新）", "deploy@build-02", &identity(MACHINE_ID_B));
        registry.update(&renamed).expect("update ssh host details");
        let after_update = registry
            .get(remote.host_id())
            .expect("get updated host")
            .expect("updated host must exist");
        assert_eq!(after_update.display_name, "构建机（新）");
        assert_eq!(after_update.ssh_target.as_deref(), Some("deploy@build-02"));
        assert_eq!(after_update.remote_data_dir, None);
        assert_eq!(after_update.machine_id_hash(), remote.machine_id_hash());

        registry
            .update_last_success(local.host_id(), 1_785_468_844_419)
            .expect("first success timestamp");
        registry
            .update_last_success(local.host_id(), 1_785_468_999_999)
            .expect("second success timestamp");
        let refreshed = registry
            .get(local.host_id())
            .expect("get refreshed host")
            .expect("refreshed host must exist");
        assert_eq!(refreshed.last_success_utc, Some(1_785_468_999_999));
        assert_eq!(registry.list().expect("list after updates").len(), 2);

        let missing = registry
            .update_last_success("0000000000000000", 1)
            .expect_err("unknown host must not be silently created");
        assert!(matches!(missing, HostError::HostNotFound { .. }));
        assert_eq!(registry.list().expect("list after failed update").len(), 2);

        registry.delete(remote.host_id()).expect("delete ssh host");
        assert!(registry
            .get(remote.host_id())
            .expect("get deleted host")
            .is_none());
        assert_eq!(registry.list().expect("list after delete").len(), 1);
        assert!(matches!(
            registry.delete(remote.host_id()),
            Err(HostError::HostNotFound { .. })
        ));

        registry
            .insert(&HostRecord::ssh(
                "重新添加",
                "deploy@build-03",
                &identity(MACHINE_ID_B),
            ))
            .expect("machine may be re-registered after deletion");
        assert_eq!(registry.list().expect("list after re-insert").len(), 2);
    }

    #[test]
    fn host_display_name_with_sql_metacharacters_is_stored_literally() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = Archive::open_in_data_dir(dir.path()).expect("open archive in tempdir");
        let registry = HostRegistry::new(archive.connection());

        let hostile = format!("{HOSTILE_DISPLAY_NAME}\n\"quoted\"");
        let hostile = hostile.as_str();
        let host = HostRecord::local(hostile, &identity(MACHINE_ID_A));
        registry
            .insert(&host)
            .expect("hostile display name inserts");

        let stored = registry
            .get(host.host_id())
            .expect("get hostile host")
            .expect("hostile host must exist");
        assert_eq!(stored.display_name, hostile);

        let table_count: i64 = archive
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'hosts'",
                [],
                |row| row.get(0),
            )
            .expect("hosts table must survive");
        assert_eq!(table_count, 1);

        let duplicate = HostRecord::local("另一台", &identity(MACHINE_ID_A));
        let error = registry
            .insert(&duplicate)
            .expect_err("duplicate must still be rejected");
        assert!(
            error.to_string().contains(hostile),
            "duplicate error must echo the stored literal display name: {error}"
        );
    }

    #[test]
    fn host_kind_encoding_round_trip() {
        assert_eq!(HostKind::Local.as_str(), "local");
        assert_eq!(HostKind::Ssh.as_str(), "ssh");
        assert_eq!(
            HostKind::from_encoded("local").expect("local"),
            HostKind::Local
        );
        assert_eq!(HostKind::from_encoded("ssh").expect("ssh"), HostKind::Ssh);
        assert_eq!(
            serde_json::to_string(&HostKind::Ssh).expect("serialize kind"),
            "\"ssh\""
        );
        let error = HostKind::from_encoded("remote").expect_err("unknown kind must fail");
        println!("invalid_kind_error={error}");
        assert!(matches!(error, HostError::InvalidHostKind { .. }));
    }

    #[test]
    fn host_registry_rejects_inconsistent_kind_and_ssh_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = Archive::open_in_data_dir(dir.path()).expect("open archive in tempdir");
        let registry = HostRegistry::new(archive.connection());

        let mut local_with_target = HostRecord::local("本机", &identity(MACHINE_ID_A));
        local_with_target.ssh_target = Some("user@host".to_owned());
        let error = registry
            .insert(&local_with_target)
            .expect_err("local host must not carry an ssh target");
        println!("local_with_ssh_target_error={error}");
        assert!(matches!(error, HostError::SshTargetOnLocalHost { .. }));

        let mut ssh_without_target = HostRecord::ssh("远端", "user@host", &identity(MACHINE_ID_B));
        ssh_without_target.ssh_target = None;
        let error = registry
            .insert(&ssh_without_target)
            .expect_err("ssh host must carry an ssh target");
        println!("ssh_without_target_error={error}");
        assert!(matches!(error, HostError::MissingSshTarget { .. }));

        let mut blank_name = HostRecord::local("   ", &identity(MACHINE_ID_A));
        blank_name.display_name = "   ".to_owned();
        let error = registry
            .insert(&blank_name)
            .expect_err("blank display name must be rejected");
        println!("blank_display_name_error={error}");
        assert!(matches!(error, HostError::BlankDisplayName));
        assert!(registry.list().expect("list hosts").is_empty());
    }

    #[test]
    #[ignore = "manual QA requires the external sha256sum and sqlite3 binaries"]
    fn host_manual_qa_external_sha256sum_and_sqlite3() {
        let dir = tempfile::tempdir().expect("tempdir");
        let directory = dir.path().to_path_buf();

        println!("--- production_chain_on_this_machine ---");
        match local_machine_id() {
            Ok(machine_id) => println!("resolved a {} character machine id", machine_id.len()),
            Err(error) => println!("no machine id available: {error}"),
        }

        let (machine_id_path, provenance) = LINUX_MACHINE_ID_SOURCES
            .iter()
            .map(Path::new)
            .find(|source| source.exists())
            .map_or_else(
                || {
                    let stand_in = directory.join("machine-id");
                    fs::write(&stand_in, format!("{MACHINE_ID_A}\n"))
                        .expect("write stand-in machine id");
                    (stand_in, "stand-in file (this host has no machine-id)")
                },
                |source| (source.to_path_buf(), "real production machine-id source"),
            );
        let machine_id =
            machine_id_from_sources(&[machine_id_path.as_path()]).expect("read machine id source");
        let rust_identity = identity(&machine_id);

        let shell = Command::new("sh")
            .arg("-c")
            .arg("printf '%s' \"$(cat \"$1\")\" | sha256sum")
            .arg("sh")
            .arg(&machine_id_path)
            .output()
            .expect("run external sha256sum");
        assert!(shell.status.success(), "sha256sum failed");
        let shell_digest = String::from_utf8_lossy(&shell.stdout)
            .split_whitespace()
            .next()
            .expect("sha256sum digest")
            .to_owned();
        println!("--- cross_tool_sha256 ---");
        println!("source={} ({provenance})", machine_id_path.display());
        println!("trimmed_machine_id_len={}", machine_id.len());
        println!("shell_sha256={shell_digest}");
        println!("rust_machine_id_hash={}", rust_identity.machine_id_hash());
        println!("shell_first16={}", &shell_digest[..HOST_ID_HEX_LENGTH]);
        println!("rust_host_id={}", rust_identity.host_id());
        assert_eq!(rust_identity.machine_id_hash(), shell_digest);
        assert_eq!(rust_identity.host_id(), &shell_digest[..HOST_ID_HEX_LENGTH]);

        let archive = Archive::open_in_data_dir(&directory).expect("open archive in tempdir");
        let path = archive.path().to_path_buf();
        let registry = HostRegistry::new(archive.connection());
        let local = HostRecord::local("本机-QA", &rust_identity);
        let remote = HostRecord::ssh("构建机-QA", "deploy@build-01", &identity(MACHINE_ID_B))
            .with_remote_data_dir(Some("/srv/opencode".to_owned()));
        let hostile = HostRecord::local(HOSTILE_DISPLAY_NAME, &identity(MACHINE_ID_C));
        registry.insert(&local).expect("insert local QA host");
        registry.insert(&remote).expect("insert ssh QA host");
        registry.insert(&hostile).expect("insert hostile QA host");
        registry
            .update_last_success(local.host_id(), 1_785_468_844_419)
            .expect("update QA last success");
        let duplicate_error = registry
            .insert(&HostRecord::ssh(
                "重复机器",
                "deploy@same-machine",
                &rust_identity,
            ))
            .expect_err("duplicate machine must be rejected");
        drop(archive);

        for (label, statement) in [
            (
                "hosts_rows",
                "SELECT host_id, display_name, kind, ssh_target, machine_id_hash FROM hosts ORDER BY display_name;",
            ),
            ("hosts_count", "SELECT COUNT(*) FROM hosts;"),
            (
                "hosts_last_success",
                "SELECT host_id, last_success_utc, remote_data_dir FROM hosts ORDER BY display_name;",
            ),
            (
                "hosts_table_survived_injection_probe",
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name;",
            ),
        ] {
            let output = Command::new("sqlite3")
                .arg("-header")
                .arg(&path)
                .arg(statement)
                .output()
                .expect("run external sqlite3");
            assert!(output.status.success(), "sqlite3 {label} failed");
            println!(
                "--- sqlite3 {label} ---\n{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        println!("--- duplicate_registration_error ---\n{duplicate_error}");

        dir.close().expect("remove manual QA tempdir");
        assert!(!directory.exists());
        println!("--- cleanup_receipt ---\nremoved {}", directory.display());
    }
}
