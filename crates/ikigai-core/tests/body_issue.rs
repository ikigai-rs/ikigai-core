//! A downstream resolver can accept a BodyIssue and collect its attachment in a default.
use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{Body, BodyIssue, Bytes, IssueContext, Result};

#[async_trait]
trait Resolver: Sync {
    async fn resolve_body(&self, mut issue: BodyIssue) -> Result<Option<(String, Bytes)>> {
        let Some((name, body)) = issue.take_attachment() else {
            return Ok(None);
        };
        let bytes = body
            .collect_within(issue.context().memory_budget().unwrap())
            .await?;
        Ok(Some((name, bytes)))
    }
}

struct CollectingResolver;
impl Resolver for CollectingResolver {}

#[test]
fn resolver_default_collects_a_one_shot_attachment() {
    let resolver: &dyn Resolver = &CollectingResolver;
    let issue = BodyIssue::new()
        .with_context(IssueContext::new().with_memory_limit(5))
        .attach(
            "content",
            Body::once(futures::stream::iter([
                Ok(Bytes::from_static(b"abc")),
                Ok(Bytes::from_static(b"de")),
            ])),
        );
    assert_eq!(
        block_on(resolver.resolve_body(issue)).unwrap(),
        Some(("content".into(), Bytes::from_static(b"abcde")))
    );
    assert_eq!(
        block_on(resolver.resolve_body(BodyIssue::new())).unwrap(),
        None
    );
}

#[test]
fn resolver_default_obeys_the_issues_memory_budget() {
    let issue = BodyIssue::new()
        .with_context(IssueContext::new().with_memory_limit(2))
        .attach(
            "content",
            Body::once(futures::stream::iter([Ok(Bytes::from_static(b"abc"))])),
        );
    assert!(matches!(
        block_on(CollectingResolver.resolve_body(issue)),
        Err(ikigai_core::Error::OverBudget { .. })
    ));
}

#[test]
fn taking_attachment_preserves_resolution_context() {
    use ikigai_core::{EndpointSpace, Expiry, Iri, Provenance, Scope, TraceEvent, Tracer};
    use std::sync::Arc;

    struct Recorder;
    impl Tracer for Recorder {
        fn record(&self, _: TraceEvent) {}
    }
    let tracer: Arc<dyn Tracer> = Arc::new(Recorder);
    let name = Iri::parse("urn:resolver:scope").unwrap();
    let incoming = Provenance::new(Expiry::Never, Default::default());
    let scope = Scope::empty().with_named(name, Arc::new(EndpointSpace::new()));
    let mut issue = BodyIssue::new()
        .in_scope(scope.clone())
        .with_incoming(incoming.clone())
        .with_context(IssueContext::new().with_memory_limit(5).with_retry_limit(2))
        .traced(Arc::clone(&tracer))
        .attach("content", Body::from(b"abc".to_vec()));

    assert_eq!(issue.attachment().map(|(name, _)| name), Some("content"));
    let (argument, body) = issue.take_attachment().unwrap();
    assert_eq!(argument, "content");
    assert_eq!(block_on(body.collect(3)).unwrap(), b"abc"[..]);
    assert!(issue.attachment().is_none());
    assert!(issue.take_attachment().is_none());
    assert_eq!(issue.scope().fingerprint(), scope.fingerprint());
    assert_eq!(issue.scope().spaces().len(), 1);
    assert_eq!(issue.incoming(), Some(&incoming));
    assert_eq!(issue.context().memory_budget().unwrap().remaining(), 5);
    assert_eq!(issue.context().retry_budget().unwrap().remaining(), 2);
    assert!(Arc::ptr_eq(issue.tracer().unwrap(), &tracer));

    let empty = BodyIssue::new();
    assert!(empty.scope().is_empty());
    assert!(empty.incoming().is_none());
    assert!(empty.tracer().is_none());
    assert!(empty.attachment().is_none());
}
