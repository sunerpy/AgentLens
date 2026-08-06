//! EXCLUSIVE FILE BOUNDARY — todo 18 owns this module.
//!
//! OS-keyring credential storage plus the DTOs for the todo-18 IPC surface
//! (credentials, the SSH connection probe and the local machine identity).
//!
//! # Why the secret never reaches a file, a log line or a DTO
//!
//! Passwords and key passphrases live **only** in the operating system's credential
//! store: Windows Credential Manager, or libsecret / the Secret Service on Linux.
//! Three structural guarantees back that up, so it is not merely a convention:
//!
//! 1. [`Secret`] has **no** `Serialize`/`Deserialize` impl, so it is impossible to
//!    place it inside any IPC DTO — that is a compile error, not a review comment.
//! 2. [`Secret`]'s `Debug` impl prints `Secret(<redacted>)`, so `{:?}` in a log or a
//!    panic message cannot leak it.
//! 3. [`CredentialStatus`] — the only credential shape that crosses IPC — carries a
//!    boolean `present` and nothing else. Reads never return the plaintext to the UI.
//!
//! # Why the store is a trait
//!
//! [`CredentialStore`] abstracts the real keyring so the round-trip, overwrite and
//! structured-absence semantics are provable with an in-memory double, deterministically
//! and without a running secret service. A headless CI container (this one included) has
//! no D-Bus secret service, so a test that talked to the real keyring would fail at
//! runtime for environmental reasons and tell us nothing about our logic. The real
//! [`OsKeyringStore`] is exercised by the `#[ignore]`d
//! `credentials_os_keyring_round_trip_requires_a_running_secret_service` test, which must
//! be run explicitly on a machine that has one.
//!
//! # DTO placement
//!
//! These DTOs deliberately live here rather than in `contract.rs`: a sibling worker edits
//! `contract.rs` concurrently, and an additive edit to a file nobody else touches cannot
//! be lost in a merge. `bindings.rs` exports them alongside the `contract.rs` types.
use std::fmt;

use agentlens_core::transport::ssh::{RemoteArchitecture, SshProbe};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Keyring service name; the account is derived from the host id and credential kind.
pub const KEYRING_SERVICE: &str = "AgentLens";

/// Separator between host id and credential kind inside the keyring account name.
const ACCOUNT_SEPARATOR: char = ':';

pub type Result<T> = std::result::Result<T, CredentialError>;

/// A plaintext secret held in memory for the shortest possible time.
///
/// Intentionally missing: `Serialize`, `Deserialize`, `Display`, and a derived `Debug`.
/// The only way to look at the bytes is [`Secret::expose`], which is easy to grep for.
#[derive(Clone, Eq, PartialEq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Read the plaintext. Every call site is a deliberate disclosure decision.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

/// Which secret a [`CredentialRef`] addresses.
///
/// `password` is an interactive login password; `passphrase` unlocks a private key file.
/// They are separate keyring entries so replacing one never clobbers the other.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    Password,
    Passphrase,
}

impl CredentialKind {
    /// The literal written into the keyring account name; part of the storage contract.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Passphrase => "passphrase",
        }
    }
}

/// Addresses one keyring entry. Never carries the secret itself.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CredentialRef {
    pub host_id: String,
    pub kind: CredentialKind,
}

impl CredentialRef {
    pub fn new(host_id: impl Into<String>, kind: CredentialKind) -> Self {
        Self {
            host_id: host_id.into(),
            kind,
        }
    }

    /// Keyring account name: `<host_id>:<kind>`.
    ///
    /// `host_id` is 16 lowercase hex characters (see `agentlens_core::host`), so it can
    /// never contain the separator and the mapping stays injective.
    pub fn account(&self) -> String {
        format!(
            "{}{ACCOUNT_SEPARATOR}{}",
            self.host_id.trim(),
            self.kind.as_str()
        )
    }

    fn validated(&self) -> Result<String> {
        if self.host_id.trim().is_empty() {
            return Err(CredentialError::BlankHostId);
        }
        Ok(self.account())
    }
}

/// The only credential shape that crosses the IPC boundary: presence, never plaintext.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub host_id: String,
    pub kind: CredentialKind,
    pub present: bool,
}

impl CredentialStatus {
    fn new(reference: &CredentialRef, present: bool) -> Self {
        Self {
            host_id: reference.host_id.clone(),
            kind: reference.kind,
            present,
        }
    }
}

/// Result of a read: a missing entry is a value, not an error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialLookup {
    Present(Secret),
    Absent,
}

impl CredentialLookup {
    pub const fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    pub const fn secret(&self) -> Option<&Secret> {
        match self {
            Self::Present(secret) => Some(secret),
            Self::Absent => None,
        }
    }
}

/// Result of a delete: removing a non-existent entry is success, not an error, so
/// re-pairing a host is idempotent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialDeletion {
    Deleted,
    Absent,
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("host_id must not be blank when addressing a keyring entry")]
    BlankHostId,
    #[error("refusing to store an empty secret; delete the entry instead")]
    EmptySecret,
    #[error("操作系统钥匙串不可用：{detail}")]
    Unavailable { detail: String },
    #[error("操作系统钥匙串拒绝了该凭据：{detail}")]
    Rejected { detail: String },
    #[error("钥匙串中的凭据不是有效的 UTF-8 文本（account={account}）")]
    BadEncoding { account: String },
    #[error("钥匙串中有多个条目匹配 account={account}")]
    Ambiguous { account: String },
}

impl CredentialError {
    /// Chinese, actionable next step. Surfaced verbatim by the hosts view.
    pub const fn remediation(&self) -> &'static str {
        match self {
            Self::BlankHostId => "请先添加或选中一台主机，再保存其口令。",
            Self::EmptySecret => "口令不能为空；若要清除已保存的口令，请使用删除按钮。",
            Self::Unavailable { .. } => {
                "请确认系统钥匙串服务正在运行（Linux 需 libsecret / gnome-keyring 已解锁；\
                 Windows 使用凭据管理器），然后重试。"
            }
            Self::Rejected { .. } => "请缩短主机名或口令长度后重试；钥匙串对条目长度有平台上限。",
            Self::BadEncoding { .. } => "该条目可能由其他程序写入。请删除这条凭据后重新保存一次。",
            Self::Ambiguous { .. } => {
                "请在系统钥匙串中删除重复的 AgentLens 条目，只保留一条后重试。"
            }
        }
    }

    /// Stable machine-readable discriminator for `IpcError::fields`.
    pub const fn variant(&self) -> &'static str {
        match self {
            Self::BlankHostId => "blankHostId",
            Self::EmptySecret => "emptySecret",
            Self::Unavailable { .. } => "unavailable",
            Self::Rejected { .. } => "rejected",
            Self::BadEncoding { .. } => "badEncoding",
            Self::Ambiguous { .. } => "ambiguous",
        }
    }
}

/// Storage backend for secrets. Implemented by the real OS keyring in production and by
/// an in-memory double in tests.
pub trait CredentialStore: Send + Sync {
    fn store(&self, reference: &CredentialRef, secret: &Secret) -> Result<CredentialStatus>;
    fn read(&self, reference: &CredentialRef) -> Result<CredentialLookup>;
    fn delete(&self, reference: &CredentialRef) -> Result<CredentialDeletion>;

    /// Presence probe that never materialises the plaintext for the caller.
    fn status(&self, reference: &CredentialRef) -> Result<CredentialStatus> {
        Ok(CredentialStatus::new(
            reference,
            self.read(reference)?.is_present(),
        ))
    }
}

/// The real OS keyring: Windows Credential Manager, or libsecret / Secret Service.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsKeyringStore;

impl OsKeyringStore {
    fn entry(account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|error| map_keyring_error(error, account))
    }
}

impl CredentialStore for OsKeyringStore {
    fn store(&self, reference: &CredentialRef, secret: &Secret) -> Result<CredentialStatus> {
        let account = reference.validated()?;
        if secret.is_empty() {
            return Err(CredentialError::EmptySecret);
        }
        // `set_password` overwrites an existing entry in place on every supported
        // platform, so re-pairing a host replaces the old secret instead of stacking a
        // second credential the reader could then pick ambiguously.
        Self::entry(&account)?
            .set_password(secret.expose())
            .map_err(|error| map_keyring_error(error, &account))?;
        Ok(CredentialStatus::new(reference, true))
    }

    fn read(&self, reference: &CredentialRef) -> Result<CredentialLookup> {
        let account = reference.validated()?;
        match Self::entry(&account)?.get_password() {
            Ok(password) => Ok(CredentialLookup::Present(Secret::new(password))),
            Err(keyring::Error::NoEntry) => Ok(CredentialLookup::Absent),
            Err(error) => Err(map_keyring_error(error, &account)),
        }
    }

    fn delete(&self, reference: &CredentialRef) -> Result<CredentialDeletion> {
        let account = reference.validated()?;
        match Self::entry(&account)?.delete_credential() {
            Ok(()) => Ok(CredentialDeletion::Deleted),
            Err(keyring::Error::NoEntry) => Ok(CredentialDeletion::Absent),
            Err(error) => Err(map_keyring_error(error, &account)),
        }
    }
}

/// `keyring::Error` is `#[non_exhaustive]`, so the catch-all arm is mandatory rather than
/// lazy: a future variant degrades to "keyring unavailable" instead of failing to compile.
fn map_keyring_error(error: keyring::Error, account: &str) -> CredentialError {
    match error {
        keyring::Error::NoEntry => CredentialError::Unavailable {
            detail: format!("account={account} 条目已消失"),
        },
        keyring::Error::BadEncoding(_) => CredentialError::BadEncoding {
            account: account.to_owned(),
        },
        keyring::Error::Ambiguous(_) => CredentialError::Ambiguous {
            account: account.to_owned(),
        },
        keyring::Error::TooLong(field, limit) => CredentialError::Rejected {
            detail: format!("{field} 超过平台上限 {limit}"),
        },
        keyring::Error::Invalid(field, reason) => CredentialError::Rejected {
            detail: format!("{field}: {reason}"),
        },
        // `PlatformFailure` / `NoStorageAccess` / future variants. The error's own
        // Display is included, never the secret (the secret is not part of the error).
        other => CredentialError::Unavailable {
            detail: other.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// SSH connection probe DTOs
// ---------------------------------------------------------------------------

/// Input for the "测试连接" button. Mirrors the add-SSH-host form fields.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SshProbeInput {
    /// `ssh` alias or `user@host`.
    pub ssh_target: String,
    /// Optional private key path (`ssh -i`).
    pub identity_file: Option<String>,
    /// Optional override for the remote OpenCode data directory.
    pub remote_data_dir: Option<String>,
}

/// The STAGE1 facts the hosts view renders on success.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SshProbeResult {
    /// Remote `uname -m` normalised to the collector architecture names.
    pub architecture: String,
    /// Discovered `XDG_DATA_HOME`, or `null` when the remote leaves it unset.
    pub xdg_data_home: Option<String>,
    /// The data directory actually used: the explicit override when given, otherwise
    /// derived from the discovered `XDG_DATA_HOME`, otherwise the documented default.
    pub data_dir: String,
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub available_kib: u64,
    /// Which machine-id source answered on the remote host.
    pub machine_id_source: String,
}

impl SshProbeResult {
    pub fn from_probe(probe: &SshProbe, remote_data_dir: Option<&str>) -> Self {
        Self {
            architecture: architecture_name(probe.architecture).to_owned(),
            xdg_data_home: probe.xdg_data_home.clone(),
            data_dir: resolve_data_dir(probe.xdg_data_home.as_deref(), remote_data_dir),
            available_kib: probe.available_kib,
            machine_id_source: probe.machine_id_source.clone(),
        }
    }
}

const fn architecture_name(architecture: RemoteArchitecture) -> &'static str {
    match architecture {
        RemoteArchitecture::X86_64 => "x86_64",
        RemoteArchitecture::Aarch64 => "aarch64",
    }
}

/// Mirrors `agentlens_core::source::opencode`'s discovery order for the remote side:
/// explicit override → `$XDG_DATA_HOME/opencode` → `~/.local/share/opencode`.
fn resolve_data_dir(xdg_data_home: Option<&str>, remote_data_dir: Option<&str>) -> String {
    if let Some(explicit) = remote_data_dir.map(str::trim).filter(|dir| !dir.is_empty()) {
        return explicit.to_owned();
    }
    match xdg_data_home.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(xdg) => format!("{}/opencode", xdg.trim_end_matches('/')),
        None => "~/.local/share/opencode".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Local machine identity
// ---------------------------------------------------------------------------

/// The auto-registration identity behind the local host card.
///
/// The frontend cannot compute this: `machine_id_hash` is SHA-256 over the trimmed
/// contents of `/etc/machine-id` (or the platform equivalent), and getting it wrong would
/// split one machine into two hosts and double-count its usage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LocalIdentity {
    pub host_id: String,
    pub machine_id_hash: String,
    /// Remote hostname when discoverable; `null` lets the view use its own default label
    /// instead of embedding user-visible text in Rust.
    pub hostname: Option<String>,
}

/// In-memory [`CredentialStore`] double.
///
/// Test-only (`#[cfg(test)]`), so it cannot leak into a production binary and needs no
/// `#[allow(dead_code)]`.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct InMemoryCredentialStore {
    entries: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    /// When set, every operation fails with this detail, so the error path is testable
    /// without a broken machine.
    failure: Option<String>,
}

#[cfg(test)]
impl InMemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn failing(detail: impl Into<String>) -> Self {
        Self {
            entries: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            failure: Some(detail.into()),
        }
    }

    /// Raw view of what was persisted, for assertions about storage layout.
    pub fn accounts(&self) -> Vec<String> {
        self.entries.lock().expect("lock").keys().cloned().collect()
    }

    fn guard(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, std::collections::BTreeMap<String, String>>> {
        if let Some(detail) = &self.failure {
            return Err(CredentialError::Unavailable {
                detail: detail.clone(),
            });
        }
        self.entries
            .lock()
            .map_err(|_| CredentialError::Unavailable {
                detail: "in-memory credential store lock poisoned".to_owned(),
            })
    }
}

#[cfg(test)]
impl CredentialStore for InMemoryCredentialStore {
    fn store(&self, reference: &CredentialRef, secret: &Secret) -> Result<CredentialStatus> {
        let account = reference.validated()?;
        if secret.is_empty() {
            return Err(CredentialError::EmptySecret);
        }
        self.guard()?.insert(account, secret.expose().to_owned());
        Ok(CredentialStatus::new(reference, true))
    }

    fn read(&self, reference: &CredentialRef) -> Result<CredentialLookup> {
        let account = reference.validated()?;
        Ok(match self.guard()?.get(&account) {
            Some(secret) => CredentialLookup::Present(Secret::new(secret.clone())),
            None => CredentialLookup::Absent,
        })
    }

    fn delete(&self, reference: &CredentialRef) -> Result<CredentialDeletion> {
        let account = reference.validated()?;
        Ok(match self.guard()?.remove(&account) {
            Some(_) => CredentialDeletion::Deleted,
            None => CredentialDeletion::Absent,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use super::*;
    use crate::contract::{Host, HostKind};

    const SECRET: &str = "correct-horse-battery-staple";
    const HOST: &str = "0123456789abcdef";

    fn password() -> CredentialRef {
        CredentialRef::new(HOST, CredentialKind::Password)
    }

    fn passphrase() -> CredentialRef {
        CredentialRef::new(HOST, CredentialKind::Passphrase)
    }

    #[test]
    fn credentials_round_trip_through_the_store_double() {
        let store = InMemoryCredentialStore::new();

        let status = store
            .store(&password(), &Secret::new(SECRET))
            .expect("store secret");
        assert_eq!(
            status,
            CredentialStatus {
                host_id: HOST.to_owned(),
                kind: CredentialKind::Password,
                present: true,
            }
        );

        let lookup = store.read(&password()).expect("read secret");
        assert_eq!(
            lookup.secret().map(Secret::expose),
            Some(SECRET),
            "the stored plaintext must come back byte-for-byte"
        );
        assert!(store.status(&password()).expect("status").present);
        assert_eq!(store.accounts(), vec![format!("{HOST}:password")]);
    }

    #[test]
    fn credentials_overwrite_replaces_the_secret_without_adding_a_second_entry() {
        let store = InMemoryCredentialStore::new();
        store
            .store(&password(), &Secret::new("first"))
            .expect("store first secret");
        store
            .store(&password(), &Secret::new("second"))
            .expect("overwrite secret");

        assert_eq!(
            store.accounts(),
            vec![format!("{HOST}:password")],
            "re-pairing must overwrite in place, never stack a duplicate credential"
        );
        assert_eq!(
            store
                .read(&password())
                .expect("read secret")
                .secret()
                .map(Secret::expose),
            Some("second")
        );
    }

    #[test]
    fn credentials_password_and_passphrase_are_independent_entries() {
        let store = InMemoryCredentialStore::new();
        store
            .store(&password(), &Secret::new("login"))
            .expect("store password");
        store
            .store(&passphrase(), &Secret::new("unlock-key"))
            .expect("store passphrase");

        assert_eq!(
            store.accounts(),
            vec![format!("{HOST}:passphrase"), format!("{HOST}:password")]
        );

        assert_eq!(
            store.delete(&password()).expect("delete password"),
            CredentialDeletion::Deleted
        );
        assert_eq!(
            store
                .read(&passphrase())
                .expect("read passphrase")
                .secret()
                .map(Secret::expose),
            Some("unlock-key"),
            "deleting the password must not touch the key passphrase"
        );
    }

    #[test]
    fn credentials_missing_entry_reads_as_structured_absence_not_an_error() {
        let store = InMemoryCredentialStore::new();

        let lookup = store
            .read(&password())
            .expect("a missing entry must not be an error");
        assert_eq!(lookup, CredentialLookup::Absent);
        assert!(!lookup.is_present());
        assert_eq!(lookup.secret(), None);
        assert!(!store.status(&password()).expect("status").present);
    }

    #[test]
    fn credentials_delete_is_idempotent_and_reports_absence() {
        let store = InMemoryCredentialStore::new();
        store
            .store(&password(), &Secret::new(SECRET))
            .expect("store secret");

        assert_eq!(
            store.delete(&password()).expect("first delete"),
            CredentialDeletion::Deleted
        );
        assert_eq!(
            store.delete(&password()).expect("second delete"),
            CredentialDeletion::Absent,
            "deleting an absent entry is success, so re-pairing never hard-fails"
        );
        assert_eq!(
            store.read(&password()).expect("read after delete"),
            CredentialLookup::Absent
        );
    }

    #[test]
    fn credentials_reject_blank_host_and_empty_secret() {
        let store = InMemoryCredentialStore::new();
        let blank = CredentialRef::new("   ", CredentialKind::Password);

        assert!(matches!(
            store.store(&blank, &Secret::new(SECRET)),
            Err(CredentialError::BlankHostId)
        ));
        assert!(matches!(
            store.read(&blank),
            Err(CredentialError::BlankHostId)
        ));
        assert!(matches!(
            store.store(&password(), &Secret::new("")),
            Err(CredentialError::EmptySecret)
        ));
        assert!(
            store.accounts().is_empty(),
            "a rejected write must not persist anything"
        );
    }

    #[test]
    fn credentials_store_failure_surfaces_chinese_remediation() {
        let store = InMemoryCredentialStore::failing("no secret service on the session bus");
        let error = store
            .store(&password(), &Secret::new(SECRET))
            .expect_err("failing store must report an error");

        assert_eq!(error.variant(), "unavailable");
        assert!(error.to_string().contains("钥匙串不可用"));
        assert!(error.remediation().contains("libsecret"));
    }

    #[test]
    fn credentials_secret_is_absent_from_every_serialized_dto() {
        let store = InMemoryCredentialStore::new();
        let status = store
            .store(&password(), &Secret::new(SECRET))
            .expect("store secret");

        let host = Host {
            host_id: HOST.to_owned(),
            machine_id_hash: "a".repeat(64),
            display_name: "workstation".to_owned(),
            kind: HostKind::Ssh,
            ssh_target: Some("ci@build-box.internal".to_owned()),
            remote_data_dir: Some("/srv/opencode".to_owned()),
            last_success_utc: Some(1),
        };
        let probe = SshProbeResult {
            architecture: "x86_64".to_owned(),
            xdg_data_home: Some("/home/ci/.local/share".to_owned()),
            data_dir: "/srv/opencode".to_owned(),
            available_kib: 1_048_576,
            machine_id_source: "/etc/machine-id".to_owned(),
        };

        for encoded in [
            serde_json::to_string(&status).expect("serialize credential status"),
            serde_json::to_string(&host).expect("serialize host"),
            serde_json::to_string(&probe).expect("serialize probe result"),
            serde_json::to_string(&password()).expect("serialize credential ref"),
        ] {
            assert!(
                !encoded.contains(SECRET),
                "serialized DTO leaked the secret: {encoded}"
            );
        }

        // `Debug` must not leak it either — panic messages and log lines use it.
        assert_eq!(format!("{:?}", Secret::new(SECRET)), "Secret(<redacted>)");
        assert!(!format!("{:?}", CredentialLookup::Present(Secret::new(SECRET))).contains(SECRET));
    }

    #[test]
    fn credentials_account_name_is_stable_and_injective() {
        assert_eq!(password().account(), format!("{HOST}:password"));
        assert_eq!(passphrase().account(), format!("{HOST}:passphrase"));
        assert_eq!(
            CredentialRef::new(format!("  {HOST}  "), CredentialKind::Password).account(),
            format!("{HOST}:password"),
            "surrounding whitespace must not create a second entry for the same host"
        );
    }

    #[test]
    fn credentials_probe_result_resolves_the_data_directory_by_discovery_order() {
        assert_eq!(
            resolve_data_dir(Some("/home/ci/.local/share"), Some("/srv/opencode")),
            "/srv/opencode",
            "an explicit override wins"
        );
        assert_eq!(
            resolve_data_dir(Some("/home/ci/.local/share/"), None),
            "/home/ci/.local/share/opencode"
        );
        assert_eq!(resolve_data_dir(None, None), "~/.local/share/opencode");
        assert_eq!(
            resolve_data_dir(Some("   "), Some("  ")),
            "~/.local/share/opencode",
            "blank values are absent values, not empty paths"
        );
    }

    #[test]
    fn credentials_probe_result_maps_stage1_facts() {
        let probe = SshProbe {
            architecture: RemoteArchitecture::Aarch64,
            xdg_data_home: Some("/home/ci/.local/share".to_owned()),
            available_kib: 4_096,
            machine_id_source: "/var/lib/dbus/machine-id".to_owned(),
        };
        let result = SshProbeResult::from_probe(&probe, None);

        assert_eq!(result.architecture, "aarch64");
        assert_eq!(result.data_dir, "/home/ci/.local/share/opencode");
        assert_eq!(result.available_kib, 4_096);
        assert_eq!(result.machine_id_source, "/var/lib/dbus/machine-id");
    }

    #[test]
    fn credentials_os_store_rejects_invalid_requests_before_opening_the_keyring() {
        let store = OsKeyringStore;
        let blank = CredentialRef::new("   ", CredentialKind::Password);

        assert!(matches!(
            store.store(&blank, &Secret::new(SECRET)),
            Err(CredentialError::BlankHostId)
        ));
        assert!(matches!(
            store.read(&blank),
            Err(CredentialError::BlankHostId)
        ));
        assert!(matches!(
            store.delete(&blank),
            Err(CredentialError::BlankHostId)
        ));
        assert!(matches!(
            store.store(&password(), &Secret::new("")),
            Err(CredentialError::EmptySecret)
        ));
    }

    #[test]
    fn credentials_os_store_can_construct_a_namespaced_entry_without_contacting_the_service() {
        let account = password().account();
        assert!(
            OsKeyringStore::entry(&account).is_ok(),
            "constructing an AgentLens entry must not require a live secret-service session"
        );
    }

    #[test]
    fn credentials_keyring_errors_map_to_stable_domain_variants_without_secrets() {
        let io_error = || {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "fixture backend failure",
            )) as Box<dyn std::error::Error + Send + Sync>
        };
        let cases = [
            (
                keyring::Error::NoEntry,
                CredentialError::Unavailable {
                    detail: "account=fixture:password 条目已消失".to_owned(),
                },
            ),
            (
                keyring::Error::BadEncoding(vec![0xff]),
                CredentialError::BadEncoding {
                    account: "fixture:password".to_owned(),
                },
            ),
            (
                keyring::Error::Ambiguous(Vec::new()),
                CredentialError::Ambiguous {
                    account: "fixture:password".to_owned(),
                },
            ),
            (
                keyring::Error::TooLong("password".to_owned(), 128),
                CredentialError::Rejected {
                    detail: "password 超过平台上限 128".to_owned(),
                },
            ),
            (
                keyring::Error::Invalid("service".to_owned(), "blank".to_owned()),
                CredentialError::Rejected {
                    detail: "service: blank".to_owned(),
                },
            ),
            (
                keyring::Error::PlatformFailure(io_error()),
                CredentialError::Unavailable {
                    detail: "Platform secure storage failure: fixture backend failure".to_owned(),
                },
            ),
        ];

        for (source, expected) in cases {
            let mapped = map_keyring_error(source, "fixture:password");
            assert_eq!(mapped.variant(), expected.variant());
            assert_eq!(mapped.to_string(), expected.to_string());
            assert!(!mapped.remediation().is_empty());
        }
    }

    #[test]
    fn credentials_every_error_variant_has_distinct_actionable_guidance() {
        let cases = [
            (CredentialError::BlankHostId, "blankHostId", "主机"),
            (CredentialError::EmptySecret, "emptySecret", "不能为空"),
            (
                CredentialError::Unavailable {
                    detail: "offline".to_owned(),
                },
                "unavailable",
                "libsecret",
            ),
            (
                CredentialError::Rejected {
                    detail: "too long".to_owned(),
                },
                "rejected",
                "长度",
            ),
            (
                CredentialError::BadEncoding {
                    account: "fixture".to_owned(),
                },
                "badEncoding",
                "重新保存",
            ),
            (
                CredentialError::Ambiguous {
                    account: "fixture".to_owned(),
                },
                "ambiguous",
                "重复",
            ),
        ];

        let mut variants = std::collections::BTreeSet::new();
        for (error, expected_variant, remediation_fragment) in cases {
            assert_eq!(error.variant(), expected_variant);
            assert!(error.remediation().contains(remediation_fragment));
            assert!(variants.insert(error.variant()), "variants must be unique");
        }
    }

    #[test]
    fn credentials_failing_and_poisoned_test_stores_surface_all_read_write_paths() {
        let failing = InMemoryCredentialStore::failing("fixture secret service unavailable");
        assert!(matches!(
            failing.store(&password(), &Secret::new(SECRET)),
            Err(CredentialError::Unavailable { ref detail })
                if detail == "fixture secret service unavailable"
        ));
        assert!(matches!(
            failing.read(&password()),
            Err(CredentialError::Unavailable { ref detail })
                if detail == "fixture secret service unavailable"
        ));
        assert!(matches!(
            failing.status(&password()),
            Err(CredentialError::Unavailable { ref detail })
                if detail == "fixture secret service unavailable"
        ));
        assert!(matches!(
            failing.delete(&password()),
            Err(CredentialError::Unavailable { ref detail })
                if detail == "fixture secret service unavailable"
        ));

        let poisoned = InMemoryCredentialStore::new();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _entries = poisoned.entries.lock().expect("lock entries");
            panic!("poison credential store lock");
        }));
        assert!(panic.is_err());
        let error = poisoned
            .read(&password())
            .expect_err("a poisoned fake store must fail rather than panic");
        assert!(matches!(
            error,
            CredentialError::Unavailable { ref detail }
                if detail == "in-memory credential store lock poisoned"
        ));
    }

    #[test]
    fn credentials_probe_maps_x86_and_preserves_discovered_home() {
        let probe = SshProbe {
            architecture: RemoteArchitecture::X86_64,
            xdg_data_home: Some("/data/home/".to_owned()),
            available_kib: 8_192,
            machine_id_source: "/etc/machine-id".to_owned(),
        };
        let result = SshProbeResult::from_probe(&probe, Some("  /explicit/data  "));

        assert_eq!(result.architecture, "x86_64");
        assert_eq!(result.xdg_data_home.as_deref(), Some("/data/home/"));
        assert_eq!(result.data_dir, "/explicit/data");
        assert_eq!(result.available_kib, 8_192);
    }

    #[test]
    fn credentials_canary_scanner_recurses_and_detects_plaintext_without_a_keyring() {
        let directory = tempfile::tempdir().expect("create scan directory");
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).expect("create nested directory");
        std::fs::write(directory.path().join("root.txt"), b"safe root")
            .expect("write root fixture");
        std::fs::write(nested.join("nested.txt"), b"safe nested").expect("write nested fixture");

        let mut searched = Vec::new();
        assert_no_plaintext(directory.path(), SECRET, &mut searched);
        searched.sort();
        assert_eq!(searched.len(), 2);
        assert!(searched.iter().all(|path| path.is_file()));

        std::fs::write(nested.join("leak.txt"), format!("prefix-{SECRET}-suffix"))
            .expect("write leak fixture");
        let detection = catch_unwind(AssertUnwindSafe(|| {
            assert_no_plaintext(directory.path(), SECRET, &mut Vec::new());
        }));
        assert!(
            detection.is_err(),
            "the scanner must reject a plaintext canary"
        );

        let mut absent = Vec::new();
        assert_no_plaintext(&directory.path().join("missing"), SECRET, &mut absent);
        assert!(absent.is_empty());

        let directories = app_directories();
        let unique: std::collections::BTreeSet<_> = directories.iter().collect();
        assert_eq!(unique.len(), directories.len());
    }

    /// Executable form of the "secrets never touch the disk" claim.
    ///
    /// Stores a canary through the production keyring path, then walks every file under
    /// the app's data and config directories asserting the plaintext appears nowhere.
    /// `#[ignore]`d for the same reason as the round-trip test: it needs a real secret
    /// service. Run with
    /// `cargo test -p agentlens-tauri credentials_canary -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires a running OS secret service (libsecret / Credential Manager)"]
    fn credentials_canary_never_appears_under_the_app_directories() {
        let canary = std::env::var("AGENTLENS_CANARY")
            .unwrap_or_else(|_| "AGENTLENS-PLAINTEXT-CANARY-9f3c1e7a".to_owned());
        let store = OsKeyringStore;
        let reference = CredentialRef::new("feedfacecafe0002", CredentialKind::Password);

        store
            .store(&reference, &Secret::new(canary.clone()))
            .expect("store canary in the OS keyring");

        let mut searched = Vec::new();
        for directory in app_directories() {
            assert_no_plaintext(&directory, &canary, &mut searched);
        }
        println!(
            "scanned {} files under {:?}",
            searched.len(),
            app_directories()
        );

        store.delete(&reference).expect("delete canary");
    }

    #[cfg(test)]
    fn app_directories() -> Vec<std::path::PathBuf> {
        let mut directories = Vec::new();
        if let Ok(archive) = agentlens_core::archive::default_archive_path() {
            if let Some(parent) = archive.parent() {
                directories.push(parent.to_path_buf());
            }
        }
        if let Ok(prices) = agentlens_core::pricing::default_prices_path() {
            if let Some(parent) = prices.parent() {
                if !directories.contains(&parent.to_path_buf()) {
                    directories.push(parent.to_path_buf());
                }
            }
        }
        directories
    }

    #[cfg(test)]
    fn assert_no_plaintext(
        directory: &std::path::Path,
        canary: &str,
        searched: &mut Vec<std::path::PathBuf>,
    ) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                assert_no_plaintext(&path, canary, searched);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            assert!(
                !bytes
                    .windows(canary.len())
                    .any(|window| window == canary.as_bytes()),
                "plaintext secret found on disk in {}",
                path.display()
            );
            searched.push(path);
        }
    }

    /// Real OS keyring round trip.
    ///
    /// `#[ignore]`d on purpose: it needs a running Secret Service / Credential Manager,
    /// which a headless container does not have. Marking it ignored keeps `cargo test`
    /// honest — a green default run makes no claim about the real keyring. Run it with
    /// `cargo test -p agentlens-tauri credentials_os_keyring -- --ignored` on a desktop
    /// session and report the actual outcome.
    #[test]
    #[ignore = "requires a running OS secret service (libsecret / Credential Manager)"]
    fn credentials_os_keyring_round_trip_requires_a_running_secret_service() {
        let store = OsKeyringStore;
        let reference = CredentialRef::new("feedfacecafe0001", CredentialKind::Passphrase);
        let _ = store.delete(&reference);

        store
            .store(&reference, &Secret::new(SECRET))
            .expect("store secret in the OS keyring");
        assert_eq!(
            store
                .read(&reference)
                .expect("read secret from the OS keyring")
                .secret()
                .map(Secret::expose),
            Some(SECRET)
        );
        assert_eq!(
            store.delete(&reference).expect("delete keyring entry"),
            CredentialDeletion::Deleted
        );
        assert_eq!(
            store.read(&reference).expect("read after delete"),
            CredentialLookup::Absent
        );
    }
}
