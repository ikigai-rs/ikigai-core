//! **The cache policy from a host's config** (ledger #2687): a `[cache]` table in a
//! host's `config.toml`, handed to `CacheConfig`, names the policy and its parameters;
//! a bad one fails loud; and `urn:kernel:cache` names the policy in force.

use futures::executor::block_on;
use ikigai_core::{
    builtins, ArgRef, CacheConfig, Capability, EndpointSpace, Exact, Iri, Kernel, Request, Verb,
};
use std::sync::Arc;

/// The `[cache]` table of a host's `config.toml`, as a host reads it.
fn cache_table(config_toml: &str) -> CacheConfig {
    #[derive(serde::Deserialize)]
    struct Host {
        #[serde(default)]
        cache: CacheConfig,
    }
    toml::from_str::<Host>(config_toml)
        .expect("config.toml parses")
        .cache
}

fn describe(config_toml: &str) -> String {
    cache_table(config_toml)
        .to_policy()
        .expect("builds")
        .describe()
}

fn refusal(config_toml: &str) -> String {
    match cache_table(config_toml).to_policy() {
        Ok(policy) => panic!("built {}", policy.describe()),
        Err(error) => error.to_string(),
    }
}

#[test]
fn an_absent_table_is_the_default_cache() {
    assert_eq!(describe(""), "lru (4096 entries, 67108864 bytes)");
    assert_eq!(describe("[cache]\n"), "lru (4096 entries, 67108864 bytes)");
}

#[test]
fn each_policy_is_named_with_its_parameters() {
    assert_eq!(
        describe("[cache]\npolicy = \"fifo\"\nmax_entries = 16\nmax_bytes = 1024\n"),
        "fifo (16 entries, 1024 bytes)"
    );
    assert_eq!(
        describe("[cache]\npolicy = \"cost-aware\"\n"),
        "cost-aware (4096 entries, 67108864 bytes)"
    );
    assert_eq!(
        describe(
            "[cache]\npolicy = \"scan-resistant\"\nmax_bytes = 536870912\nprotected_percent = 60\n"
        ),
        "scan-resistant (4096 entries, 536870912 bytes, protected 60%)"
    );
}

#[test]
fn partitions_take_their_budget_out_of_the_whole() {
    let config = r#"
[cache]
policy = "scan-resistant"
max_entries = 1000
max_bytes = 1000000

[[cache.partition]]
prefix = "urn:feed:"
max_bytes = 600000

[[cache.partition]]
prefix = "urn:tenant:b/"
max_bytes = 100000
max_entries = 50
policy = "lru"
"#;
    assert_eq!(
        describe(config),
        "partitioned (\
         urn:feed:* scan-resistant (600 entries, 600000 bytes, protected 80%); \
         urn:tenant:b/* lru (50 entries, 100000 bytes); \
         rest scan-resistant (350 entries, 300000 bytes, protected 80%))"
    );
    let policy = cache_table(config).to_policy().unwrap();
    assert_eq!(policy.capacity().max_bytes, 1_000_000, "the whole is kept");
    assert_eq!(policy.capacity().max_entries, 1000);
}

#[test]
fn an_unknown_policy_fails_loud_and_names_the_known_ones() {
    assert_eq!(
        refusal("[cache]\npolicy = \"lfu\"\n"),
        "cache config: unknown cache policy \"lfu\" at policy \
         (known: lru, fifo, cost-aware, scan-resistant)"
    );
    assert_eq!(
        refusal(
            "[cache]\n[[cache.partition]]\nprefix = \"urn:feed:\"\nmax_bytes = 1\npolicy = \"arc\"\n"
        ),
        "cache config: unknown cache policy \"arc\" at partition[0].policy \
         (known: lru, fifo, cost-aware, scan-resistant)"
    );
}

#[test]
fn a_bad_value_fails_loud_and_names_its_key() {
    for (config, expected) in [
        (
            "[cache]\nmax_entries = 0\n",
            "max_entries must be at least 1",
        ),
        ("[cache]\nmax_bytes = 0\n", "max_bytes must be at least 1"),
        (
            "[cache]\npolicy = \"scan-resistant\"\nprotected_percent = 100\n",
            "protected_percent = 100 is not between 1 and 99",
        ),
        (
            "[cache]\nprotected_percent = 50\n",
            "protected_percent is a scan-resistant setting, and the policy is \"lru\"",
        ),
        (
            "[cache]\n[[cache.partition]]\nprefix = \"\"\nmax_bytes = 1\n",
            "partition[0].prefix is empty",
        ),
        (
            "[cache]\n[[cache.partition]]\nprefix = \"a:\"\nmax_bytes = 1\n\
             [[cache.partition]]\nprefix = \"a:\"\nmax_bytes = 1\n",
            "partition[1].prefix \"a:\" is given twice",
        ),
        (
            "[cache]\nmax_bytes = 100\n[[cache.partition]]\nprefix = \"a:\"\nmax_bytes = 100\n",
            "the partitions take the whole cache",
        ),
    ] {
        let message = refusal(config);
        assert!(message.contains(expected), "{config}: {message}");
    }
}

#[test]
fn an_unknown_key_or_a_wrong_type_is_refused_by_the_parse() {
    for config in [
        "[cache]\nmax_size = 10\n",
        "[cache]\nmax_bytes = \"64MiB\"\n",
        "[cache]\nmax_bytes = -1\n",
        "[cache]\n[[cache.partition]]\nprefix = \"a:\"\n",
    ] {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Host {
            cache: CacheConfig,
        }
        assert!(toml::from_str::<Host>(config).is_err(), "{config}");
    }
}

#[test]
fn the_configured_policy_reaches_the_kernel_and_its_readout() {
    let policy =
        cache_table("[cache]\npolicy = \"scan-resistant\"\nmax_entries = 2\nmax_bytes = 1048576\n")
            .to_policy()
            .unwrap();
    let space = EndpointSpace::new().bind(Exact::new("urn:test:to-upper"), builtins::to_upper());
    let kernel = Kernel::new(Arc::new(space)).with_cache_policy(policy);
    let cap = Capability::root();
    let read = |word: &str| {
        Request::new(Verb::Source, Iri::parse("urn:test:to-upper").unwrap())
            .with_arg("in", ArgRef::Inline(word.as_bytes().to_vec()))
    };
    for word in ["a", "b", "c"] {
        block_on(kernel.issue(read(word), &cap)).unwrap();
    }
    assert_eq!(kernel.cache_len(), 2, "the configured bound is enforced");
    let out = block_on(kernel.issue(
        Request::new(Verb::Source, Iri::parse("urn:kernel:cache").unwrap()),
        &cap,
    ))
    .unwrap();
    let text = String::from_utf8(out.bytes).unwrap();
    assert!(
        text.contains("  policy   scan-resistant (2 entries, 1048576 bytes, protected 80%)\n"),
        "{text}"
    );
    assert!(
        text.contains("  evictions  0\n"),
        "a declined newcomer evicts nothing: {text}"
    );
}
