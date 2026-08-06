//! 手工价格覆盖表与成本估算（todo 9）。
//!
//! 本模块以 `data_dir/agentlens/prices.json` 作为价格的**唯一事实源**（没有对应 SQL 表），
//! 条目形状为 `{ provider_id, model_id, input_per_mtok, output_per_mtok,
//! cache_read_per_mtok, cache_write_per_mtok }`；写入必须原子（写临时文件后 rename）。
//!
//! 对 `cost_source = unavailable` 且命中价格表的记录，在查询期动态计算 `estimated` 成本，
//! 不回写归档库的原始 `cost` 列；未命中则保持 `unavailable`。
//! `estimated` 与 `actual` 分开返回，绝不混加。
//!
//! 结构上预留将来由 models.dev 拉取填充同一文件，本期不联网。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::archive::{CostSource, NormalizedUsageRecord};

/// Data-directory subdirectory shared with the archive database.
const PRICES_DIRECTORY: &str = "agentlens";
/// File name of the single price source of truth.
const PRICES_FILE: &str = "prices.json";
/// Prefix of the same-directory temporary file used by the atomic write.
///
/// The leading dot plus this exact prefix is what makes a leftover temporary file impossible to
/// confuse with the real document: [`PriceTable::load`] only ever opens [`PRICES_FILE`].
const TEMP_FILE_PREFIX: &str = ".prices.json.tmp-";
/// Divisor behind every `*_per_mtok` price: prices are quoted per 1,000,000 tokens.
const TOKENS_PER_MTOK: f64 = 1_000_000.0;
const READ_MAX_ATTEMPTS: usize = 8;
const READ_RETRY_DELAY: Duration = Duration::from_millis(1);
const READ_RETRY_LIMIT: Duration = Duration::from_millis(20);

/// Document version understood by this build.
///
/// The field exists so a future models.dev importer can rewrite the same file with a richer layout
/// while older builds still fail loudly instead of silently mis-reading it.
pub const PRICES_SCHEMA_VERSION: u32 = 1;

fn is_transient_read_error(source: &io::Error) -> bool {
    let transient = source.kind() == io::ErrorKind::PermissionDenied;
    #[cfg(windows)]
    let transient = transient || source.raw_os_error() == Some(32);
    transient
}

/// Result type returned by pricing operations.
pub type Result<T> = std::result::Result<T, PricingError>;

/// Errors returned while resolving, loading, validating, or atomically writing `prices.json`.
#[derive(Debug, Error)]
pub enum PricingError {
    /// The operating system did not expose a per-user data directory.
    #[error("cannot resolve the user data directory for AgentLens prices.json")]
    DataDirectoryUnavailable,
    /// The prices path has no usable parent directory.
    #[error("prices file path is invalid: {0}")]
    InvalidPricesPath(PathBuf),
    /// The prices directory could not be created.
    #[error("cannot prepare prices directory at {path}: {source}")]
    Directory {
        /// Directory involved in the failed operation.
        path: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
    /// The prices file exists but could not be read.
    #[error("cannot read prices file at {path}: {source}")]
    Read {
        /// Prices file that could not be read.
        path: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
    /// The prices file is not valid JSON, or not a valid price document.
    #[error(
        "prices file at {path} is not valid AgentLens price JSON: {source}; fix the file by hand or delete it to run without price overrides"
    )]
    Parse {
        /// Prices file that failed to parse.
        path: PathBuf,
        /// Original JSON error, including its line and column.
        source: serde_json::Error,
    },
    /// The document declares a `schema_version` this build does not understand.
    #[error(
        "prices file schema_version {found} is not supported by this build (supported: {supported}); upgrade AgentLens instead of editing the version by hand"
    )]
    UnsupportedSchema {
        /// Version found in the document.
        found: u32,
        /// Version understood by this build.
        supported: u32,
    },
    /// A price is negative or non-finite.
    #[error(
        "price entry {provider_id}/{model_id} has an invalid {field}: {value}; every per-Mtok price must be finite and must not be negative"
    )]
    InvalidPrice {
        /// Provider of the offending entry.
        provider_id: String,
        /// Model of the offending entry.
        model_id: String,
        /// Offending field name.
        field: &'static str,
        /// Offending value.
        value: f64,
    },
    /// A lookup-key component is empty or whitespace only.
    #[error(
        "price entry at index {index} has a blank {field}; provider_id and model_id form the lookup key and must be non-empty"
    )]
    BlankIdentifier {
        /// Zero-based index of the offending entry.
        index: usize,
        /// Offending field name.
        field: &'static str,
    },
    /// Two entries share the same lookup key.
    #[error(
        "price table contains more than one entry for {provider_id}/{model_id}; the lookup key (provider_id, model_id) must be unique"
    )]
    DuplicateEntry {
        /// Duplicated provider.
        provider_id: String,
        /// Duplicated model.
        model_id: String,
    },
    /// The in-memory table could not be serialized.
    #[error("cannot serialize the price table: {source}")]
    Serialize {
        /// Original JSON error.
        source: serde_json::Error,
    },
    /// The temporary file could not be created, written, or flushed to disk.
    #[error("cannot write the temporary prices file at {path}: {source}")]
    Write {
        /// Temporary file path.
        path: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
    /// The temporary file could not replace the target, so nothing was changed.
    #[error("cannot atomically replace {target} with {temp}: {source}")]
    Rename {
        /// Fully written temporary file (already removed again).
        temp: PathBuf,
        /// Intended target path, left untouched.
        target: PathBuf,
        /// Original filesystem error.
        source: std::io::Error,
    },
}

/// Billable token buckets that feed a cost estimate.
///
/// Reasoning tokens are deliberately absent: providers disagree on whether `tok_reasoning` is a
/// subset of output or a separate bucket, the document has no `reasoning_per_mtok` field, and
/// guessing either way would silently double-count or under-count. Excluding them from the type
/// makes a wrong wiring impossible rather than merely discouraged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenCounts {
    /// Cache-miss input tokens, priced with `input_per_mtok`.
    pub input: u64,
    /// Output tokens, priced with `output_per_mtok`.
    pub output: u64,
    /// Cache-read input tokens, priced with `cache_read_per_mtok`.
    pub cache_read: u64,
    /// Cache-write input tokens, priced with `cache_write_per_mtok`.
    pub cache_write: u64,
}

impl From<&NormalizedUsageRecord> for TokenCounts {
    /// Copies the four billable buckets straight off an archive record.
    ///
    /// `tok_input` is the cache-MISS input count, so the derived `total_input`
    /// (`input + cache_read + cache_write`) must never be substituted here: doing so would charge
    /// the full input price on cached tokens as well.
    fn from(record: &NormalizedUsageRecord) -> Self {
        Self {
            input: record.tok_input,
            output: record.tok_output,
            cache_read: record.tok_cache_read,
            cache_write: record.tok_cache_write,
        }
    }
}

/// One manual price override, quoted per 1,000,000 tokens in the user's own currency.
///
/// Unknown JSON fields are preserved in [`PriceEntry::extra`] so a future models.dev importer can
/// annotate entries (provenance, timestamps) without older builds dropping the annotations on the
/// next save.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PriceEntry {
    /// Provider identifier, matched verbatim against `usage_record.provider_id`.
    pub provider_id: String,
    /// Model identifier, matched verbatim against `usage_record.model_id`.
    pub model_id: String,
    /// Price of cache-miss input tokens per 1,000,000 tokens.
    pub input_per_mtok: f64,
    /// Price of output tokens per 1,000,000 tokens.
    pub output_per_mtok: f64,
    /// Price of cache-read input tokens per 1,000,000 tokens.
    pub cache_read_per_mtok: f64,
    /// Price of cache-write input tokens per 1,000,000 tokens.
    pub cache_write_per_mtok: f64,
    /// Forward-compatibility passthrough for unknown entry-level fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl PriceEntry {
    /// Builds an entry with no forward-compatibility extras.
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        input_per_mtok: f64,
        output_per_mtok: f64,
        cache_read_per_mtok: f64,
        cache_write_per_mtok: f64,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            input_per_mtok,
            output_per_mtok,
            cache_read_per_mtok,
            cache_write_per_mtok,
            extra: BTreeMap::new(),
        }
    }

    /// Computes the cost of one record's token buckets.
    ///
    /// The mapping is fixed and each bucket is charged exactly once:
    /// `input → input_per_mtok`, `output → output_per_mtok`,
    /// `cache_read → cache_read_per_mtok`, `cache_write → cache_write_per_mtok`.
    /// Every bucket contributes `tokens / 1_000_000 * per_mtok`; reasoning tokens are not priced.
    pub fn estimate(&self, tokens: TokenCounts) -> f64 {
        Self::bucket_cost(tokens.input, self.input_per_mtok)
            + Self::bucket_cost(tokens.output, self.output_per_mtok)
            + Self::bucket_cost(tokens.cache_read, self.cache_read_per_mtok)
            + Self::bucket_cost(tokens.cache_write, self.cache_write_per_mtok)
    }

    fn bucket_cost(tokens: u64, per_mtok: f64) -> f64 {
        tokens as f64 / TOKENS_PER_MTOK * per_mtok
    }

    fn validate(&self, index: usize) -> Result<()> {
        for (field, value) in [
            ("provider_id", self.provider_id.as_str()),
            ("model_id", self.model_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(PricingError::BlankIdentifier { index, field });
            }
        }

        for (field, value) in [
            ("input_per_mtok", self.input_per_mtok),
            ("output_per_mtok", self.output_per_mtok),
            ("cache_read_per_mtok", self.cache_read_per_mtok),
            ("cache_write_per_mtok", self.cache_write_per_mtok),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(PricingError::InvalidPrice {
                    provider_id: self.provider_id.clone(),
                    model_id: self.model_id.clone(),
                    field,
                    value,
                });
            }
        }

        Ok(())
    }
}

/// Cost of one record after the price table has been consulted.
///
/// The three states stay distinct all the way to the caller; there is deliberately no accessor that
/// collapses [`ResolvedCost::Actual`] and [`ResolvedCost::Estimated`] into a single number, and
/// [`ResolvedCost::Unavailable`] never degrades to `0.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedCost {
    /// Trustworthy source-reported cost.
    Actual(f64),
    /// Cost derived from a local price override at query time; never written back to the archive.
    Estimated(f64),
    /// Neither a source cost nor a price override exists.
    Unavailable,
}

impl ResolvedCost {
    /// Returns the [`CostSource`] this resolution corresponds to.
    pub const fn source(self) -> CostSource {
        match self {
            Self::Actual(_) => CostSource::Actual,
            Self::Estimated(_) => CostSource::Estimated,
            Self::Unavailable => CostSource::Unavailable,
        }
    }

    /// Returns the actual cost, or `None` for estimated and unavailable rows.
    pub const fn actual(self) -> Option<f64> {
        match self {
            Self::Actual(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the estimated cost, or `None` for actual and unavailable rows.
    pub const fn estimated(self) -> Option<f64> {
        match self {
            Self::Estimated(value) => Some(value),
            _ => None,
        }
    }
}

/// Layered cost aggregate: actual and estimated money never share a field.
///
/// Field names match the fixture manifest and todo 8's summary contract
/// (`cost: {actual_sum, unavailable_count, estimated_sum}`). Rows without any cost only ever
/// increment `unavailable_count`; they are never added as `0.0`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CostTotals {
    /// Sum of trustworthy source-reported costs.
    pub actual_sum: f64,
    /// Sum of query-time estimates produced from the price table.
    pub estimated_sum: f64,
    /// Number of rows with neither an actual cost nor a price override.
    pub unavailable_count: u64,
}

impl CostTotals {
    /// Folds one resolved cost into the matching layer.
    pub fn add(&mut self, resolved: ResolvedCost) {
        match resolved {
            ResolvedCost::Actual(value) => self.actual_sum += value,
            ResolvedCost::Estimated(value) => self.estimated_sum += value,
            ResolvedCost::Unavailable => self.unavailable_count += 1,
        }
    }
}

/// The whole `prices.json` document: the single source of truth for manual price overrides.
///
/// # Lookup key
///
/// Entries are keyed by `(provider_id, model_id)` and **`variant` is deliberately ignored**. The
/// archive's model key is `(provider_id, model_id, variant)`, but real prices vary by model, not by
/// reasoning-effort variant, so `xhigh`, `low`, and a missing variant all resolve to the same entry.
/// Downstream code (todos 8 and 19) must not add a variant column to the price editor.
///
/// # On-disk shape
///
/// ```json
/// {
///   "schema_version": 1,
///   "entries": [
///     {
///       "provider_id": "kiro-auth",
///       "model_id": "claude-opus-5-max",
///       "input_per_mtok": 3.0,
///       "output_per_mtok": 15.0,
///       "cache_read_per_mtok": 0.3,
///       "cache_write_per_mtok": 3.75
///     }
///   ]
/// }
/// ```
///
/// Field names are snake_case exactly as written here, because the file is meant to be edited by
/// hand (and by todo 19's editor). All six entry fields are required: a missing price is a readable
/// error rather than a silent `0`. Unknown fields at document and entry level are preserved across a
/// load/save round trip so a later models.dev importer can enrich the same file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PriceTable {
    /// Document version; must equal [`PRICES_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Manual price overrides, in the order the file lists them.
    pub entries: Vec<PriceEntry>,
    /// Forward-compatibility passthrough for unknown document-level fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for PriceTable {
    fn default() -> Self {
        Self {
            schema_version: PRICES_SCHEMA_VERSION,
            entries: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl PriceTable {
    /// Returns an empty table stamped with the current schema version.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a table built from `entries`, stamped with the current schema version.
    pub fn from_entries(entries: Vec<PriceEntry>) -> Self {
        Self {
            schema_version: PRICES_SCHEMA_VERSION,
            entries,
            extra: BTreeMap::new(),
        }
    }

    /// Checks the document contract: known schema version, non-blank keys, finite non-negative
    /// prices, and at most one entry per `(provider_id, model_id)`.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PRICES_SCHEMA_VERSION {
            return Err(PricingError::UnsupportedSchema {
                found: self.schema_version,
                supported: PRICES_SCHEMA_VERSION,
            });
        }

        let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
        for (index, entry) in self.entries.iter().enumerate() {
            entry.validate(index)?;
            if !seen.insert((entry.provider_id.as_str(), entry.model_id.as_str())) {
                return Err(PricingError::DuplicateEntry {
                    provider_id: entry.provider_id.clone(),
                    model_id: entry.model_id.clone(),
                });
            }
        }

        Ok(())
    }

    /// Finds the entry for `(provider_id, model_id)`, ignoring any model variant.
    ///
    /// The scan is linear because a hand-maintained table holds a handful of models; callers that
    /// resolve millions of rows may hoist their own map if profiling ever demands it.
    pub fn lookup(&self, provider_id: &str, model_id: &str) -> Option<&PriceEntry> {
        self.entries
            .iter()
            .find(|entry| entry.provider_id == provider_id && entry.model_id == model_id)
    }

    /// Estimates a cost for `(provider_id, model_id)`, or `None` when the model is not priced.
    pub fn estimate(&self, provider_id: &str, model_id: &str, tokens: TokenCounts) -> Option<f64> {
        self.lookup(provider_id, model_id)
            .map(|entry| entry.estimate(tokens))
    }

    /// Resolves one row's cost at query time.
    ///
    /// A stored value is trusted only when it is present, finite, and declared `actual` or
    /// `estimated`. Anything else (including the contradictory `unavailable` + value combination)
    /// falls through to the price table: a hit yields [`ResolvedCost::Estimated`], a miss stays
    /// [`ResolvedCost::Unavailable`]. Nothing here writes to the archive.
    pub fn resolve_cost(
        &self,
        provider_id: &str,
        model_id: &str,
        tokens: TokenCounts,
        cost: Option<f64>,
        cost_source: CostSource,
    ) -> ResolvedCost {
        let trusted = cost.filter(|value| value.is_finite());
        match (trusted, cost_source) {
            (Some(value), CostSource::Actual) => ResolvedCost::Actual(value),
            (Some(value), CostSource::Estimated) => ResolvedCost::Estimated(value),
            _ => match self.lookup(provider_id, model_id) {
                Some(entry) => ResolvedCost::Estimated(entry.estimate(tokens)),
                None => ResolvedCost::Unavailable,
            },
        }
    }

    /// Resolves one archive record's cost without touching the archive.
    pub fn resolve_record(&self, record: &NormalizedUsageRecord) -> ResolvedCost {
        self.resolve_cost(
            &record.provider_id,
            &record.model_id,
            TokenCounts::from(record),
            record.cost,
            record.cost_source,
        )
    }

    /// Loads and validates the document at an explicit path.
    ///
    /// A missing file is not an error: it means no manual overrides exist yet, so an empty table is
    /// returned. A present but malformed file is a hard [`PricingError`]; use
    /// [`PriceTable::load_or_empty`] to keep running with estimation disabled instead.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        Self::load_with_reader(path, |path| fs::read(path), thread::sleep)
    }

    fn load_with_reader<R, W>(path: &Path, mut reader: R, mut wait: W) -> Result<Self>
    where
        R: FnMut(&Path) -> io::Result<Vec<u8>>,
        W: FnMut(Duration),
    {
        let started = Instant::now();
        let mut attempts = 0;
        let bytes = loop {
            attempts += 1;
            match reader(path) {
                Ok(bytes) => break bytes,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    return Ok(Self::new());
                }
                Err(source)
                    if is_transient_read_error(&source)
                        && attempts < READ_MAX_ATTEMPTS
                        && started.elapsed() < READ_RETRY_LIMIT =>
                {
                    // MoveFileEx replacement leaves the old destination delete-pending only until
                    // its last reader handle closes, normally a sub-millisecond window. Eight open
                    // attempts spaced by 1 ms cover that window generously, while the independent
                    // 20 ms wall cap prevents scheduler delays from turning a real ACL failure into
                    // a noticeable stall. A failure that persists still returns the last typed
                    // PricingError::Read unchanged; this retry adds latency, never hides the error.
                    let remaining = READ_RETRY_LIMIT.saturating_sub(started.elapsed());
                    wait(READ_RETRY_DELAY.min(remaining));
                    if started.elapsed() >= READ_RETRY_LIMIT {
                        return Err(PricingError::Read {
                            path: path.to_path_buf(),
                            source,
                        });
                    }
                }
                Err(source) => {
                    return Err(PricingError::Read {
                        path: path.to_path_buf(),
                        source,
                    });
                }
            }
        };

        let table: Self = serde_json::from_slice(&bytes).map_err(|source| PricingError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        table.validate()?;
        Ok(table)
    }

    /// Loads `agentlens/prices.json` below an injected data directory.
    pub fn load_in_data_dir(data_dir: impl AsRef<Path>) -> Result<Self> {
        Self::load(prices_path_in(data_dir))
    }

    /// Loads the standard `dirs::data_dir()/agentlens/prices.json` document.
    pub fn load_default() -> Result<Self> {
        Self::load(default_prices_path()?)
    }

    /// Loads the document, degrading to an empty table when it cannot be used.
    ///
    /// The returned error is meant to be surfaced to the user (and logged) while the application
    /// keeps running with estimation disabled: a hand-edited typo must never crash the app.
    pub fn load_or_empty(path: impl AsRef<Path>) -> (Self, Option<PricingError>) {
        match Self::load(path) {
            Ok(table) => (table, None),
            Err(error) => (Self::new(), Some(error)),
        }
    }

    /// Validates, then atomically writes the document to an explicit path.
    ///
    /// The document is serialized to a same-directory temporary file, flushed with `fsync`, and
    /// then `rename`d over the target, so a concurrent reader always observes either the previous
    /// or the next complete document and never a partial one. An invalid table is rejected before
    /// anything is written, and a failure at any step removes the temporary file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let mut body = serde_json::to_string_pretty(self)
            .map_err(|source| PricingError::Serialize { source })?;
        body.push('\n');
        write_atomically(path.as_ref(), body.as_bytes())
    }

    /// Atomically writes `agentlens/prices.json` below an injected data directory.
    pub fn save_in_data_dir(&self, data_dir: impl AsRef<Path>) -> Result<()> {
        self.save(prices_path_in(data_dir))
    }

    /// Atomically writes the standard `dirs::data_dir()/agentlens/prices.json` document.
    pub fn save_default(&self) -> Result<()> {
        self.save(default_prices_path()?)
    }
}

/// Resolves the standard prices path without creating directories or touching the file.
pub fn default_prices_path() -> Result<PathBuf> {
    dirs::data_dir()
        .map(prices_path_in)
        .ok_or(PricingError::DataDirectoryUnavailable)
}

/// Builds an injectable prices path below an arbitrary data directory.
pub fn prices_path_in(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join(PRICES_DIRECTORY).join(PRICES_FILE)
}

fn write_atomically(path: &Path, body: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| PricingError::InvalidPricesPath(path.to_path_buf()))?;
    fs::create_dir_all(directory).map_err(|source| PricingError::Directory {
        path: directory.to_path_buf(),
        source,
    })?;

    let temp_path = directory.join(temp_file_name());
    if let Err(source) = write_and_sync(&temp_path, body) {
        let _ = fs::remove_file(&temp_path);
        return Err(PricingError::Write {
            path: temp_path,
            source,
        });
    }

    if let Err(source) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(PricingError::Rename {
            temp: temp_path,
            target: path.to_path_buf(),
            source,
        });
    }

    // Best effort: flush the directory entry so the rename itself survives a power loss.
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }

    Ok(())
}

fn write_and_sync(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(body)?;
    file.sync_all()
}

/// Builds a temporary file name unique across processes and across threads of one process.
fn temp_file_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{TEMP_FILE_PREFIX}{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::Origin;
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use tempfile::tempdir;

    /// Per-Mtok prices chosen so that every bucket contributes a DISTINCT amount; a swapped or
    /// mis-wired bucket therefore cannot produce the expected total by accident.
    const INPUT_PER_MTOK: f64 = 3.00;
    const OUTPUT_PER_MTOK: f64 = 15.00;
    const CACHE_READ_PER_MTOK: f64 = 0.30;
    const CACHE_WRITE_PER_MTOK: f64 = 3.75;

    const TOK_INPUT: u64 = 1_000_000;
    const TOK_OUTPUT: u64 = 250_000;
    const TOK_CACHE_READ: u64 = 4_000_000;
    const TOK_CACHE_WRITE: u64 = 500_000;
    const TOK_REASONING: u64 = 7_777_777;

    fn priced_entry() -> PriceEntry {
        PriceEntry::new(
            "kiro-auth",
            "claude-opus-5-max",
            INPUT_PER_MTOK,
            OUTPUT_PER_MTOK,
            CACHE_READ_PER_MTOK,
            CACHE_WRITE_PER_MTOK,
        )
    }

    fn priced_table() -> PriceTable {
        PriceTable::from_entries(vec![priced_entry()])
    }

    /// Unavailable-cost record on the priced model, carrying all four billable buckets plus a large
    /// reasoning count that must never reach the estimate.
    fn unavailable_record() -> NormalizedUsageRecord {
        NormalizedUsageRecord {
            host_id: "host-local".to_string(),
            source: "opencode".to_string(),
            message_id: "msg_pricing_1".to_string(),
            session_id: "ses_pricing".to_string(),
            time_created_utc: 1_785_468_844_419,
            time_completed_utc: Some(1_785_468_845_000),
            source_time_updated: 1_785_468_845_000,
            origin: Origin::Live,
            origin_priority: Origin::Live.priority(),
            agent_raw: "Sisyphus".to_string(),
            agent_key: "sisyphus".to_string(),
            provider_id: "kiro-auth".to_string(),
            model_id: "claude-opus-5-max".to_string(),
            variant: Some("xhigh".to_string()),
            tok_input: TOK_INPUT,
            tok_output: TOK_OUTPUT,
            tok_reasoning: TOK_REASONING,
            tok_cache_read: TOK_CACHE_READ,
            tok_cache_write: TOK_CACHE_WRITE,
            cost: None,
            cost_source: CostSource::Unavailable,
            is_incomplete: false,
            project_dir: "/config/workspace/ProdDir/AI/AgentLens".to_string(),
        }
    }

    fn write_raw(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().expect("prices path has a parent"))
            .expect("create prices directory");
        fs::write(path, body).expect("write raw prices document");
    }

    fn temp_file_names(directory: &Path) -> BTreeSet<String> {
        fs::read_dir(directory)
            .expect("read prices directory")
            .map(|entry| {
                entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.starts_with(TEMP_FILE_PREFIX))
            .collect()
    }

    #[test]
    fn pricing_estimates_unavailable_hit_with_literal_arithmetic() {
        let table = priced_table();
        let resolved = table.resolve_record(&unavailable_record());

        // Hand-computed arithmetic, written out bucket by bucket: tokens / 1_000_000 * per-Mtok.
        let expected_input = 1_000_000.0 / 1_000_000.0 * 3.00; // 3.000
        let expected_output = 250_000.0 / 1_000_000.0 * 15.00; // 3.750
        let expected_cache_read = 4_000_000.0 / 1_000_000.0 * 0.30; // 1.200
        let expected_cache_write = 500_000.0 / 1_000_000.0 * 3.75; // 1.875
        let expected =
            expected_input + expected_output + expected_cache_read + expected_cache_write;

        assert_eq!(resolved, ResolvedCost::Estimated(expected));
        assert_eq!(resolved.source(), CostSource::Estimated);
        // Literal cross-check: 3.000 + 3.750 + 1.200 + 1.875 = 9.825.
        assert_eq!(expected, 9.825);
        assert_eq!(resolved.estimated(), Some(9.825));
        assert_eq!(resolved.actual(), None);

        // Guard against charging the input price on the derived total_input
        // (input + cache_read + cache_write = 5_500_000), which would triple-charge cached tokens:
        // 5.5 * 3.00 + 3.75 = 20.25.
        assert_ne!(resolved.estimated(), Some(20.25));
    }

    #[test]
    fn pricing_estimate_maps_each_token_bucket_to_its_own_price() {
        let entry = priced_entry();

        let only_input = TokenCounts {
            input: TOK_INPUT,
            ..TokenCounts::default()
        };
        let only_output = TokenCounts {
            output: TOK_OUTPUT,
            ..TokenCounts::default()
        };
        let only_cache_read = TokenCounts {
            cache_read: TOK_CACHE_READ,
            ..TokenCounts::default()
        };
        let only_cache_write = TokenCounts {
            cache_write: TOK_CACHE_WRITE,
            ..TokenCounts::default()
        };

        assert_eq!(entry.estimate(only_input), 3.000);
        assert_eq!(entry.estimate(only_output), 3.750);
        assert_eq!(entry.estimate(only_cache_read), 1.200);
        assert_eq!(entry.estimate(only_cache_write), 1.875);
        assert_eq!(entry.estimate(TokenCounts::default()), 0.0);
    }

    #[test]
    fn pricing_estimate_excludes_reasoning_tokens() {
        let table = priced_table();
        let mut without_reasoning = unavailable_record();
        without_reasoning.tok_reasoning = 0;
        let mut with_reasoning = unavailable_record();
        with_reasoning.tok_reasoning = u64::from(u32::MAX);

        assert_eq!(
            table.resolve_record(&without_reasoning),
            table.resolve_record(&with_reasoning)
        );
        assert_eq!(
            table.resolve_record(&with_reasoning),
            ResolvedCost::Estimated(9.825)
        );
    }

    #[test]
    fn pricing_miss_keeps_row_unavailable() {
        let table = priced_table();

        let mut other_provider = unavailable_record();
        other_provider.provider_id = "myopenai".to_string();
        let mut other_model = unavailable_record();
        other_model.model_id = "gpt-nonexistent".to_string();

        for record in [&other_provider, &other_model] {
            let resolved = table.resolve_record(record);
            assert_eq!(resolved, ResolvedCost::Unavailable);
            assert_eq!(resolved.source(), CostSource::Unavailable);
            // No silent zero and no fabricated estimate.
            assert_eq!(resolved.actual(), None);
            assert_eq!(resolved.estimated(), None);
        }

        assert!(PriceTable::new()
            .lookup("kiro-auth", "claude-opus-5-max")
            .is_none());
    }

    #[test]
    fn pricing_totals_keep_actual_and_estimated_separate() {
        let table = priced_table();

        let mut actual_row = unavailable_record();
        actual_row.cost = Some(0.05);
        actual_row.cost_source = CostSource::Actual;
        let estimated_row = unavailable_record();
        let mut missing_row = unavailable_record();
        missing_row.model_id = "unpriced-model".to_string();

        let mut totals = CostTotals::default();
        for record in [&actual_row, &estimated_row, &missing_row] {
            totals.add(table.resolve_record(record));
        }

        assert_eq!(totals.actual_sum, 0.05);
        assert_eq!(totals.estimated_sum, 9.825);
        assert_eq!(totals.unavailable_count, 1);
        // The two sums are reported side by side and never folded together.
        assert_ne!(totals.actual_sum, totals.actual_sum + totals.estimated_sum);
    }

    #[test]
    fn pricing_totals_reproduce_fixture_mixed_cost_day() {
        // fixture (todo 2) mixed-cost day: one actual 0.0102 row and one unavailable row that the
        // price table does not cover; todo 8 asserts exactly this shape.
        let table = PriceTable::new();

        let mut actual_row = unavailable_record();
        actual_row.cost = Some(0.0102);
        actual_row.cost_source = CostSource::Actual;
        let unavailable_row = unavailable_record();

        let mut totals = CostTotals::default();
        totals.add(table.resolve_record(&actual_row));
        totals.add(table.resolve_record(&unavailable_row));

        assert_eq!(totals.actual_sum, 0.0102);
        assert_eq!(totals.unavailable_count, 1);
        assert_eq!(totals.estimated_sum, 0.0);
    }

    #[test]
    fn pricing_lookup_ignores_variant() {
        let table = priced_table();
        let mut xhigh = unavailable_record();
        xhigh.variant = Some("xhigh".to_string());
        let mut low = unavailable_record();
        low.variant = Some("low".to_string());
        let mut none = unavailable_record();
        none.variant = None;

        for record in [&xhigh, &low, &none] {
            assert_eq!(
                table.resolve_record(record),
                ResolvedCost::Estimated(9.825),
                "variant must not participate in the lookup key"
            );
        }
    }

    #[test]
    fn pricing_atomic_write_is_never_observed_half_written() {
        let dir = tempdir().expect("tempdir");
        let path = Arc::new(prices_path_in(dir.path()));

        let document_a = PriceTable::from_entries(vec![PriceEntry::new(
            "kiro-auth",
            "model-a",
            1.0,
            2.0,
            0.5,
            0.25,
        )]);
        let document_b = PriceTable::from_entries(vec![
            PriceEntry::new("kiro-auth", "model-b", 9.0, 90.0, 0.9, 0.09),
            PriceEntry::new("myopenai", "model-b2", 8.0, 80.0, 0.8, 0.08),
        ]);
        document_a.save(path.as_ref()).expect("seed document A");

        let known = Arc::new(vec![document_a.clone(), document_b.clone()]);
        let mut handles = Vec::new();

        for writer in 0..4 {
            let path = Arc::clone(&path);
            let document = if writer % 2 == 0 {
                document_a.clone()
            } else {
                document_b.clone()
            };
            handles.push(thread::spawn(move || {
                for _ in 0..150 {
                    document.save(path.as_ref()).expect("atomic save");
                }
            }));
        }
        for _ in 0..4 {
            let path = Arc::clone(&path);
            let known = Arc::clone(&known);
            handles.push(thread::spawn(move || {
                for _ in 0..400 {
                    let observed = PriceTable::load(path.as_ref())
                        .expect("a reader must never observe a half-written document");
                    assert!(
                        known.contains(&observed),
                        "reader observed a document that was never written: {observed:?}"
                    );
                }
            }));
        }
        for handle in handles {
            handle.join().expect("worker thread");
        }

        let directory = path.parent().expect("prices parent directory");
        assert!(
            temp_file_names(directory).is_empty(),
            "atomic writes must not leave temp files behind"
        );
        assert_eq!(
            fs::read_dir(directory).expect("read dir").count(),
            1,
            "only prices.json may remain in the directory"
        );
        assert!(known.contains(&PriceTable::load(path.as_ref()).expect("final load")));
    }

    #[test]
    fn pricing_load_retries_transient_reader_errors_until_success() {
        let path = Path::new("injected-prices.json");
        let expected = priced_table();
        let bytes = serde_json::to_vec(&expected).expect("serialize injected document");
        let mut attempts = 0;

        let loaded = PriceTable::load_with_reader(
            path,
            |_| {
                attempts += 1;
                if attempts <= 3 {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("transient attempt {attempts}"),
                    ))
                } else {
                    Ok(bytes.clone())
                }
            },
            |_| {},
        )
        .expect("transient reader failures must recover");

        assert_eq!(loaded, expected);
        assert_eq!(attempts, 4);
    }

    #[test]
    fn pricing_load_bounds_permanently_transient_reader_errors() {
        let path = Path::new("injected-prices.json");
        let mut attempts = 0;

        let error = PriceTable::load_with_reader(
            path,
            |_| {
                attempts += 1;
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("transient attempt {attempts}"),
                ))
            },
            |_| {},
        )
        .expect_err("a permanently denied reader must remain a typed error");

        assert_eq!(
            attempts, READ_MAX_ATTEMPTS,
            "transient retries must have a hard bound"
        );
        match error {
            PricingError::Read {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert_eq!(
                    source.to_string(),
                    format!("transient attempt {READ_MAX_ATTEMPTS}")
                );
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn pricing_load_does_not_retry_not_found() {
        let path = Path::new("injected-prices.json");
        let mut attempts = 0;

        let loaded = PriceTable::load_with_reader(
            path,
            |_| {
                attempts += 1;
                Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
            },
            |_| {},
        )
        .expect("a missing price file means no overrides");

        assert_eq!(attempts, 1);
        assert_eq!(loaded, PriceTable::new());
    }

    #[test]
    fn pricing_load_does_not_retry_non_transient_reader_errors() {
        let path = Path::new("injected-prices.json");
        let mut attempts = 0;

        let error = PriceTable::load_with_reader(
            path,
            |_| {
                attempts += 1;
                Err(io::Error::new(io::ErrorKind::InvalidData, "hard failure"))
            },
            |_| {},
        )
        .expect_err("non-transient reader errors must fail immediately");

        assert_eq!(attempts, 1);
        assert!(matches!(error, PricingError::Read { .. }));
    }

    #[cfg(windows)]
    #[test]
    fn pricing_windows_sharing_violation_is_transient() {
        let source = io::Error::from_raw_os_error(32);
        assert!(is_transient_read_error(&source));
    }

    #[test]
    fn pricing_load_rejects_malformed_documents_with_readable_errors() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());

        let cases = [
            (
                "truncated",
                r#"{"schema_version": 1, "entries": [{"provider_id": "kiro-auth""#,
                "EOF while parsing",
            ),
            (
                "wrong type",
                r#"{"schema_version": "one", "entries": []}"#,
                "invalid type",
            ),
            (
                "price as string",
                r#"{"schema_version": 1, "entries": [{"provider_id":"a","model_id":"b","input_per_mtok":"3","output_per_mtok":1,"cache_read_per_mtok":1,"cache_write_per_mtok":1}]}"#,
                "invalid type",
            ),
            (
                "trailing garbage",
                r#"{"schema_version": 1, "entries": []} }}}"#,
                "trailing characters",
            ),
            (
                "missing schema_version",
                r#"{"entries": []}"#,
                "missing field `schema_version`",
            ),
            (
                "missing price field",
                r#"{"schema_version": 1, "entries": [{"provider_id":"a","model_id":"b","input_per_mtok":3,"output_per_mtok":1,"cache_read_per_mtok":1}]}"#,
                "missing field `cache_write_per_mtok`",
            ),
        ];

        for (label, body, expected_fragment) in cases {
            write_raw(&path, body);
            let error = PriceTable::load(&path).expect_err(label);
            assert!(
                matches!(error, PricingError::Parse { .. }),
                "{label}: unexpected error {error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("prices.json"),
                "{label}: error must name the file: {message}"
            );
            assert!(
                message.contains(expected_fragment),
                "{label}: error must explain the problem ({expected_fragment}): {message}"
            );
            assert!(
                message.contains("delete it to run without price overrides"),
                "{label}: error must tell the user how to recover: {message}"
            );
        }
    }

    #[test]
    fn pricing_load_or_empty_disables_estimation_on_malformed_document() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        write_raw(&path, "{ this is not json");

        let (table, error) = PriceTable::load_or_empty(&path);
        let error = error.expect("malformed document must be reported");
        assert!(matches!(error, PricingError::Parse { .. }));
        assert!(table.entries.is_empty());
        assert_eq!(table.schema_version, PRICES_SCHEMA_VERSION);
        // System keeps running with estimation disabled instead of crashing.
        assert_eq!(
            table.resolve_record(&unavailable_record()),
            ResolvedCost::Unavailable
        );
    }

    #[test]
    fn pricing_load_rejects_negative_and_non_finite_prices() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        write_raw(
            &path,
            r#"{"schema_version": 1, "entries": [{"provider_id":"kiro-auth","model_id":"m","input_per_mtok":-1.0,"output_per_mtok":1,"cache_read_per_mtok":1,"cache_write_per_mtok":1}]}"#,
        );
        let error = PriceTable::load(&path).expect_err("negative price must be rejected");
        assert!(
            matches!(
                &error,
                PricingError::InvalidPrice { field, value, .. }
                    if *field == "input_per_mtok" && *value == -1.0
            ),
            "unexpected error {error:?}"
        );
        assert!(error.to_string().contains("negative"));

        // Non-finite prices can never reach the estimator either.
        let infinite = PriceTable::from_entries(vec![PriceEntry::new(
            "kiro-auth",
            "m",
            f64::INFINITY,
            1.0,
            1.0,
            1.0,
        )]);
        assert!(matches!(
            infinite.validate(),
            Err(PricingError::InvalidPrice { .. })
        ));
        // save() validates first, so an invalid table is never persisted.
        let unsaved = prices_path_in(dir.path().join("nested"));
        assert!(infinite.save(&unsaved).is_err());
        assert!(!unsaved.exists());
    }

    #[test]
    fn pricing_load_accepts_large_finite_prices() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        write_raw(
            &path,
            r#"{"schema_version": 1, "entries": [{"provider_id":"kiro-auth","model_id":"m","input_per_mtok":1e308,"output_per_mtok":0,"cache_read_per_mtok":0,"cache_write_per_mtok":0}]}"#,
        );
        let table = PriceTable::load(&path).expect("large finite prices are the user's choice");
        let estimate = table
            .lookup("kiro-auth", "m")
            .expect("entry")
            .estimate(TokenCounts {
                input: 1,
                ..TokenCounts::default()
            });
        assert!(estimate.is_finite());
        assert_eq!(estimate, 1.0 / 1_000_000.0 * 1e308);
    }

    #[test]
    fn pricing_load_rejects_duplicate_and_blank_entries() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());

        let duplicate = PriceTable::from_entries(vec![priced_entry(), priced_entry()]);
        assert!(matches!(
            duplicate.validate(),
            Err(PricingError::DuplicateEntry { .. })
        ));

        write_raw(
            &path,
            r#"{"schema_version": 1, "entries": [{"provider_id":"  ","model_id":"m","input_per_mtok":1,"output_per_mtok":1,"cache_read_per_mtok":1,"cache_write_per_mtok":1}]}"#,
        );
        let error = PriceTable::load(&path).expect_err("blank provider must be rejected");
        assert!(
            matches!(&error, PricingError::BlankIdentifier { field, .. } if *field == "provider_id"),
            "unexpected error {error:?}"
        );
    }

    #[test]
    fn pricing_load_rejects_unsupported_schema_version() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());

        for version in [0u32, PRICES_SCHEMA_VERSION + 1] {
            write_raw(
                &path,
                &format!(r#"{{"schema_version": {version}, "entries": []}}"#),
            );
            let error = PriceTable::load(&path).expect_err("unsupported schema version");
            assert!(
                matches!(
                    error,
                    PricingError::UnsupportedSchema { found, supported }
                        if found == version && supported == PRICES_SCHEMA_VERSION
                ),
                "version {version} produced {error:?}"
            );
        }
    }

    #[test]
    fn pricing_document_round_trip_preserves_unknown_fields() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        write_raw(
            &path,
            r#"{
  "schema_version": 1,
  "generated_by": "models.dev",
  "generated_at_utc": 1785468844419,
  "entries": [
    {
      "provider_id": "kiro-auth",
      "model_id": "claude-opus-5-max",
      "input_per_mtok": 3.0,
      "output_per_mtok": 15.0,
      "cache_read_per_mtok": 0.3,
      "cache_write_per_mtok": 3.75,
      "source_url": "https://models.dev/example"
    }
  ]
}
"#,
        );

        let table = PriceTable::load(&path).expect("unknown fields must be tolerated");
        assert_eq!(
            table
                .extra
                .get("generated_by")
                .and_then(|value| value.as_str()),
            Some("models.dev")
        );
        assert_eq!(
            table.entries[0]
                .extra
                .get("source_url")
                .and_then(|value| value.as_str()),
            Some("https://models.dev/example")
        );
        assert_eq!(
            table.resolve_record(&unavailable_record()),
            ResolvedCost::Estimated(9.825)
        );

        table.save(&path).expect("re-save");
        let reloaded = PriceTable::load(&path).expect("reload");
        assert_eq!(
            reloaded, table,
            "a save/load round trip must not drop fields"
        );
    }

    #[test]
    fn pricing_load_missing_file_yields_empty_table() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        assert!(!path.exists());

        let table = PriceTable::load(&path).expect("a missing file simply means no overrides");
        assert_eq!(table.schema_version, PRICES_SCHEMA_VERSION);
        assert!(table.entries.is_empty());
        assert_eq!(
            table.resolve_record(&unavailable_record()),
            ResolvedCost::Unavailable
        );

        let (fallback, error) = PriceTable::load_or_empty(&path);
        assert!(error.is_none());
        assert_eq!(fallback, table);
    }

    #[test]
    fn pricing_overwrite_is_visible_and_removed_entry_stops_estimating() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());

        priced_table().save(&path).expect("first save");
        assert_eq!(
            PriceTable::load(&path)
                .expect("load after first save")
                .resolve_record(&unavailable_record()),
            ResolvedCost::Estimated(9.825)
        );

        let doubled = PriceTable::from_entries(vec![PriceEntry::new(
            "kiro-auth",
            "claude-opus-5-max",
            INPUT_PER_MTOK * 2.0,
            OUTPUT_PER_MTOK * 2.0,
            CACHE_READ_PER_MTOK * 2.0,
            CACHE_WRITE_PER_MTOK * 2.0,
        )]);
        doubled.save(&path).expect("overwrite");
        assert_eq!(
            PriceTable::load(&path)
                .expect("load after overwrite")
                .resolve_record(&unavailable_record()),
            ResolvedCost::Estimated(9.825 * 2.0)
        );

        PriceTable::new().save(&path).expect("remove the entry");
        assert_eq!(
            PriceTable::load(&path)
                .expect("load after removal")
                .resolve_record(&unavailable_record()),
            ResolvedCost::Unavailable
        );
    }

    #[test]
    fn pricing_save_cleans_up_temp_file_when_rename_fails() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        // A directory sitting at the target path makes the final rename fail deterministically.
        fs::create_dir_all(&path).expect("create blocking directory");

        let error = priced_table()
            .save(&path)
            .expect_err("rename onto a directory must fail");
        assert!(
            matches!(error, PricingError::Rename { .. }),
            "unexpected error {error:?}"
        );
        assert!(path.is_dir(), "the pre-existing entry must survive");

        let directory = path.parent().expect("prices parent directory");
        assert!(
            temp_file_names(directory).is_empty(),
            "a failed save must not leave an orphan temp file"
        );
    }

    #[test]
    fn pricing_orphan_temp_file_never_poisons_a_load() {
        let dir = tempdir().expect("tempdir");
        let path = prices_path_in(dir.path());
        priced_table().save(&path).expect("save the real document");

        // Exactly the on-disk state left by a process killed between temp-write and rename.
        let orphan = path
            .parent()
            .expect("prices parent directory")
            .join(format!("{TEMP_FILE_PREFIX}999999-0"));
        fs::write(&orphan, "{ truncated garbage").expect("write orphan temp");

        let table = PriceTable::load(&path).expect("the real document is still intact");
        assert_eq!(table, priced_table());
        assert_eq!(
            table.resolve_record(&unavailable_record()),
            ResolvedCost::Estimated(9.825)
        );

        // A later successful save still works and does not touch the foreign temp file.
        PriceTable::new().save(&path).expect("later save");
        assert!(orphan.exists());
        assert!(PriceTable::load(&path).expect("load").entries.is_empty());
    }

    #[test]
    fn pricing_path_helpers_target_data_dir_subdirectory() {
        let dir = tempdir().expect("tempdir");
        assert_eq!(
            prices_path_in(dir.path()),
            dir.path().join("agentlens").join("prices.json")
        );
        let default_path = default_prices_path().expect("this platform exposes a data directory");
        assert!(default_path.ends_with(Path::new("agentlens").join("prices.json")));
    }

    #[test]
    fn pricing_resolve_passes_actual_cost_through_untouched() {
        let table = priced_table();
        let mut record = unavailable_record();
        record.cost = Some(0.0102);
        record.cost_source = CostSource::Actual;

        // The model IS priced; an actual cost must still win and must not be re-estimated.
        let resolved = table.resolve_record(&record);
        assert_eq!(resolved, ResolvedCost::Actual(0.0102));
        assert_eq!(resolved.actual(), Some(0.0102));
        assert_eq!(resolved.estimated(), None);

        record.cost_source = CostSource::Estimated;
        assert_eq!(
            table.resolve_record(&record),
            ResolvedCost::Estimated(0.0102)
        );
    }

    #[test]
    fn pricing_resolve_treats_missing_actual_value_as_unavailable() {
        let empty = PriceTable::new();
        let table = priced_table();

        let mut declared_actual_without_value = unavailable_record();
        declared_actual_without_value.cost = None;
        declared_actual_without_value.cost_source = CostSource::Actual;
        let mut non_finite_value = unavailable_record();
        non_finite_value.cost = Some(f64::NAN);
        non_finite_value.cost_source = CostSource::Actual;
        let mut unavailable_with_stray_value = unavailable_record();
        unavailable_with_stray_value.cost = Some(0.5);

        for record in [
            &declared_actual_without_value,
            &non_finite_value,
            &unavailable_with_stray_value,
        ] {
            assert_eq!(
                empty.resolve_record(record),
                ResolvedCost::Unavailable,
                "no trustworthy value and no price entry means unavailable"
            );
            assert_eq!(
                table.resolve_record(record),
                ResolvedCost::Estimated(9.825),
                "no trustworthy value plus a price entry means estimated"
            );
        }
    }

    #[test]
    fn pricing_data_dir_wrappers_round_trip_and_direct_estimate_preserves_partial_semantics() {
        let dir = tempdir().expect("tempdir");
        let table = priced_table();

        table
            .save_in_data_dir(dir.path())
            .expect("save through data-directory wrapper");
        let loaded =
            PriceTable::load_in_data_dir(dir.path()).expect("load through data-directory wrapper");
        assert_eq!(loaded, table);
        assert_eq!(
            loaded.estimate(
                "kiro-auth",
                "claude-opus-5-max",
                TokenCounts {
                    input: TOK_INPUT,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                }
            ),
            Some(3.0)
        );
        assert_eq!(
            loaded.estimate(
                "kiro-auth",
                "missing-model",
                TokenCounts {
                    input: TOK_INPUT,
                    ..TokenCounts::default()
                }
            ),
            None,
            "a missing price must remain unavailable rather than silently filling zero"
        );
    }

    #[test]
    fn pricing_validation_identifies_each_blank_key_and_invalid_price_bucket() {
        let mut blank_model = priced_entry();
        blank_model.model_id = " \t ".to_string();
        assert!(matches!(
            PriceTable::from_entries(vec![blank_model]).validate(),
            Err(PricingError::BlankIdentifier {
                index: 0,
                field: "model_id"
            })
        ));

        for (field, value) in [
            ("input_per_mtok", -1.0),
            ("output_per_mtok", f64::INFINITY),
            ("cache_read_per_mtok", f64::NEG_INFINITY),
            ("cache_write_per_mtok", f64::NAN),
        ] {
            let mut entry = priced_entry();
            match field {
                "input_per_mtok" => entry.input_per_mtok = value,
                "output_per_mtok" => entry.output_per_mtok = value,
                "cache_read_per_mtok" => entry.cache_read_per_mtok = value,
                "cache_write_per_mtok" => entry.cache_write_per_mtok = value,
                _ => unreachable!(),
            }
            let error = PriceTable::from_entries(vec![entry])
                .validate()
                .expect_err("invalid bucket price must fail validation");
            assert!(
                matches!(error, PricingError::InvalidPrice { field: actual, .. } if actual == field),
                "unexpected validation error for {field}: {error}"
            );
        }
    }

    #[test]
    fn pricing_save_reports_invalid_paths_and_directory_creation_failures() {
        let error = priced_table()
            .save(Path::new("prices.json"))
            .expect_err("a parentless relative path must fail");
        assert!(matches!(
            error,
            PricingError::InvalidPricesPath(path) if path == Path::new("prices.json")
        ));

        let dir = tempdir().expect("tempdir");
        let blocking_file = dir.path().join("not-a-directory");
        fs::write(&blocking_file, b"occupied").expect("write blocking file");
        let target = blocking_file.join("prices.json");
        let error = priced_table()
            .save(&target)
            .expect_err("a file cannot be prepared as the prices directory");
        assert!(matches!(
            error,
            PricingError::Directory { path, .. } if path == blocking_file
        ));
        assert_eq!(
            fs::read(&blocking_file).expect("read preserved blocker"),
            b"occupied"
        );
    }

    #[test]
    fn pricing_transient_read_retry_honors_wall_clock_limit() {
        let path = Path::new("injected-prices.json");
        let mut attempts = 0;
        let mut waits = 0;
        let error = PriceTable::load_with_reader(
            path,
            |_| {
                attempts += 1;
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "persistent transient failure",
                ))
            },
            |_| {
                waits += 1;
                thread::sleep(READ_RETRY_LIMIT + Duration::from_millis(1));
            },
        )
        .expect_err("elapsed retry budget must return the latest typed read error");

        assert_eq!(attempts, 1);
        assert_eq!(waits, 1);
        assert!(matches!(
            error,
            PricingError::Read { path: error_path, source }
                if error_path == path && source.kind() == io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    #[ignore = "manual QA: set AGENTLENS_QA_PRICES_DIR, run in background, then SIGKILL mid-write"]
    fn pricing_manual_qa_write_loop() {
        let Some(data_dir) = std::env::var_os("AGENTLENS_QA_PRICES_DIR") else {
            eprintln!("AGENTLENS_QA_PRICES_DIR is unset; nothing to do");
            return;
        };
        let path = prices_path_in(PathBuf::from(data_dir));
        let mut price = 1.0f64;
        while price <= 200_000.0 {
            let table = PriceTable::from_entries(vec![PriceEntry::new(
                "kiro-auth",
                "qa-loop",
                price,
                15.0,
                0.3,
                3.75,
            )]);
            table.save(&path).expect("atomic save must succeed");
            if price % 500.0 == 0.0 {
                eprintln!("qa write loop wrote input_per_mtok={price}");
            }
            price += 1.0;
        }
    }
}
