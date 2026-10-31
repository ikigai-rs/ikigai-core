//! The representation cache's policy, chosen by a host's config (ledger #2687).
//!
//! A host reads its `config.toml` and hands the `[cache]` table to [`CacheConfig`]
//! (it is `Deserialize`, so `toml::from_str` or any serde format will do); core reads
//! no file, as `crate::config` reads none. [`CacheConfig::to_policy`] turns it into the
//! policy `Kernel::with_cache_policy` installs, and fails loud: an unknown policy, an
//! unknown key, a zero bound or a share out of range is an error naming the key, not a
//! default quietly taken. [`CacheConfig`] shows the table.

use std::fmt;
use std::sync::Arc;

use serde::Deserialize;

use crate::cache::{CacheBound, CachePolicy, CostAware, Fifo, Lru, Partitioned, ScanResistant};

/// The policies a config can name, in the order an error lists them.
pub const CACHE_POLICIES: [&str; 4] = ["lru", "fifo", "cost-aware", "scan-resistant"];

/// A host's `[cache]` table: which policy, over what bound, with which partitions.
/// Every key is optional; an empty table is the default cache ([`Lru`] over
/// [`CacheBound::default`]). Unknown keys are refused.
///
/// ```toml
/// [cache]
/// policy = "scan-resistant"     # lru (the default) | fifo | cost-aware | scan-resistant
/// max_entries = 16384           # the whole cache, partitions included
/// max_bytes = 536870912         # 512 MiB
/// protected_percent = 80        # scan-resistant only
///
/// [[cache.partition]]           # a budget of its own for every IRI under a prefix
/// prefix = "urn:feed:"
/// max_bytes = 268435456         # taken out of the whole; the rest keeps what is left
/// max_entries = 8192            # optional: by default, its byte share of the entries
/// policy = "scan-resistant"     # optional: by default, the cache's own
/// ```
///
/// ```
/// use ikigai_core::CacheConfig;
///
/// let config: CacheConfig = serde_json::from_str(r#"{"policy": "scan-resistant"}"#).unwrap();
/// assert!(config.to_policy().unwrap().describe().starts_with("scan-resistant"));
///
/// let typo: CacheConfig = serde_json::from_str(r#"{"policy": "lfu"}"#).unwrap();
/// let refused = typo.to_policy().err().expect("an unknown policy is refused");
/// assert!(refused.to_string().contains("unknown cache policy \"lfu\""));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CacheConfig {
    /// The policy's name: one of [`CACHE_POLICIES`]. `lru` when absent.
    #[serde(default)]
    pub policy: Option<String>,
    /// The most entries the whole cache may hold, partitions included.
    #[serde(default)]
    pub max_entries: Option<u64>,
    /// The most bytes the whole cache may hold, partitions included.
    #[serde(default)]
    pub max_bytes: Option<u64>,
    /// The share of the bound protected entries may hold, `1..=99`; `scan-resistant` only.
    #[serde(default)]
    pub protected_percent: Option<u64>,
    /// Budgets of their own for names under a prefix, taken out of the whole.
    #[serde(default, rename = "partition")]
    pub partitions: Vec<PartitionConfig>,
}

/// One `[[cache.partition]]`: a prefix, the bytes it may hold, and optionally its own
/// entry count, policy and protected share.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PartitionConfig {
    /// The IRI prefix whose entries this budget holds.
    pub prefix: String,
    /// The most bytes the partition may hold. Required: a partition is a byte budget.
    pub max_bytes: u64,
    /// The most entries it may hold; by default its byte share of the cache's entries.
    #[serde(default)]
    pub max_entries: Option<u64>,
    /// Its policy; by default the cache's own.
    #[serde(default)]
    pub policy: Option<String>,
    /// Its protected share, `scan-resistant` only; by default the cache's own.
    #[serde(default)]
    pub protected_percent: Option<u64>,
}

/// Why a `[cache]` table does not describe a policy: the key and what is wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheConfigError {
    message: String,
}

impl CacheConfigError {
    fn new(message: impl Into<String>) -> Self {
        CacheConfigError {
            message: message.into(),
        }
    }
}

impl fmt::Display for CacheConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cache config: {}", self.message)
    }
}

impl std::error::Error for CacheConfigError {}

/// A positive count from a config key, as a `usize`.
fn positive(key: &str, value: u64) -> Result<usize, CacheConfigError> {
    if value == 0 {
        return Err(CacheConfigError::new(format!("{key} must be at least 1")));
    }
    usize::try_from(value)
        .map_err(|_| CacheConfigError::new(format!("{key} = {value} does not fit this platform")))
}

/// The named policy over `bound`.
fn policy(
    key: &str,
    name: &str,
    bound: CacheBound,
    protected_percent: Option<u64>,
) -> Result<Arc<dyn CachePolicy>, CacheConfigError> {
    if protected_percent.is_some() && name != "scan-resistant" {
        return Err(CacheConfigError::new(format!(
            "{key}protected_percent is a scan-resistant setting, and the policy is \"{name}\""
        )));
    }
    Ok(match name {
        "lru" => Arc::new(Lru::with_bound(bound)),
        "fifo" => Arc::new(Fifo::with_bound(bound)),
        "cost-aware" => Arc::new(CostAware::with_bound(bound)),
        "scan-resistant" => {
            let mut policy = ScanResistant::with_bound(bound);
            if let Some(percent) = protected_percent {
                let percent = u8::try_from(percent)
                    .ok()
                    .filter(|percent| (1..=99).contains(percent))
                    .ok_or_else(|| {
                        CacheConfigError::new(format!(
                            "{key}protected_percent = {percent} is not between 1 and 99"
                        ))
                    })?;
                policy = policy.protecting(percent);
            }
            Arc::new(policy)
        }
        unknown => {
            return Err(CacheConfigError::new(format!(
                "unknown cache policy \"{unknown}\" at {key}policy (known: {})",
                CACHE_POLICIES.join(", ")
            )))
        }
    })
}

impl CacheConfig {
    /// The policy this table describes, or why it describes none.
    ///
    /// Without partitions, the named policy over the bound. With them, a [`Partitioned`]
    /// policy whose whole bound is `max_entries`/`max_bytes`: each partition takes its
    /// budget out of it and the rest keeps what is left, so partitions that would leave
    /// the rest nothing are an error.
    pub fn to_policy(&self) -> Result<Arc<dyn CachePolicy>, CacheConfigError> {
        let default = CacheBound::default();
        let total = CacheBound::new(
            match self.max_entries {
                Some(value) => positive("max_entries", value)?,
                None => default.max_entries,
            },
            match self.max_bytes {
                Some(value) => positive("max_bytes", value)?,
                None => default.max_bytes,
            },
        );
        let name = self.policy.as_deref().unwrap_or("lru");
        if self.partitions.is_empty() {
            return policy("", name, total, self.protected_percent);
        }
        // Checked before any partition is, so an unknown name is reported as itself.
        policy("", name, total, self.protected_percent)?;

        let mut rest = total;
        let mut built: Vec<(String, Arc<dyn CachePolicy>)> = Vec::new();
        for (index, partition) in self.partitions.iter().enumerate() {
            let key = format!("partition[{index}].");
            if partition.prefix.is_empty() {
                return Err(CacheConfigError::new(format!(
                    "{key}prefix is empty; the rest is every name no prefix claims"
                )));
            }
            if built.iter().any(|(prefix, _)| *prefix == partition.prefix) {
                return Err(CacheConfigError::new(format!(
                    "{key}prefix \"{}\" is given twice",
                    partition.prefix
                )));
            }
            let bytes = positive(&format!("{key}max_bytes"), partition.max_bytes)?;
            let entries = match partition.max_entries {
                Some(value) => positive(&format!("{key}max_entries"), value)?,
                None => usize::try_from(
                    total.max_entries as u128 * bytes as u128 / total.max_bytes as u128,
                )
                .unwrap_or(usize::MAX)
                .max(1),
            };
            rest = CacheBound::new(
                rest.max_entries.saturating_sub(entries),
                rest.max_bytes.saturating_sub(bytes),
            );
            let protected = partition.protected_percent.or(self.protected_percent);
            let partition_name = partition.policy.as_deref().unwrap_or(name);
            // The cache's protected share is inherited only by a partition it applies to.
            let protected =
                if partition.protected_percent.is_none() && partition_name != "scan-resistant" {
                    None
                } else {
                    protected
                };
            built.push((
                partition.prefix.clone(),
                policy(
                    &key,
                    partition_name,
                    CacheBound::new(entries, bytes),
                    protected,
                )?,
            ));
        }
        if rest.max_entries == 0 || rest.max_bytes == 0 {
            return Err(CacheConfigError::new(format!(
                "the partitions take the whole cache ({} entries, {} bytes), leaving nothing \
                 for the names no prefix claims; raise max_entries/max_bytes",
                total.max_entries, total.max_bytes
            )));
        }
        let mut partitioned = Partitioned::new(policy("", name, rest, self.protected_percent)?);
        for (prefix, policy) in built {
            partitioned = partitioned.partition(prefix, policy);
        }
        Ok(Arc::new(partitioned))
    }
}
