//! File-backed replay handles must not displace small derived answers by file length.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Body, ByteStream, CacheBound, Capability, Endpoint, EndpointSpace, Exact,
    Invocation, Iri, Kernel, Lru, Replay, ReprType, Representation, Request, Result, Verb,
};

struct FileBody(PathBuf);
impl Replay for FileBody {
    fn open(&self) -> ByteStream {
        let path = self.0.clone();
        Box::pin(futures::stream::once(async move {
            Ok(std::fs::read(path).unwrap().into())
        }))
    }

    fn len_hint(&self) -> Option<u64> {
        Some(std::fs::metadata(&self.0).unwrap().len())
    }
}

impl Drop for FileBody {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

struct FileEndpoint(Arc<FileBody>);

#[async_trait::async_trait]
impl Endpoint for FileEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        self.invoke_body(inv)
            .await?
            .collect_within(inv.memory_budget().unwrap())
            .await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        Ok(Representation::from_body(
            ReprType::new("text/plain"),
            Body::replay(self.0.clone() as Arc<dyn Replay>),
        )
        .cacheable())
    }
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, Iri::parse(target).unwrap())
}

#[test]
fn file_replays_leave_room_for_cached_compositions() {
    // Test both a file larger than the bound and three files that individually fit
    // but together exceed it. The latter exposes eviction of derived answers.
    for bound in [64, 6144] {
        let runs = Arc::new(AtomicUsize::new(0));
        let mut space = EndpointSpace::new();
        for n in 0..3 {
            let path = std::env::current_dir().unwrap().join(format!(
                "replay-cache-{}-{bound}-{n}.tmp",
                std::process::id()
            ));
            std::fs::write(&path, vec![b'x'; 4096]).unwrap();
            let file = Arc::new(FileBody(path));
            let endpoint = FileEndpoint(file);
            let file_iri = format!("urn:file:{n}");
            let counter = runs.clone();
            let composition = AsyncFnEndpoint::new("summary", move |inv| {
                let file_iri = file_iri.clone();
                let counter = counter.clone();
                Box::pin(async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let answer = inv.source_body(&Iri::parse(file_iri).unwrap()).await?;
                    let bytes = answer.bytes.collect(8192).await?;
                    assert_eq!(bytes.len(), 4096);
                    Ok(
                        Representation::new(ReprType::new("text/plain"), b"small".to_vec())
                            .cacheable(),
                    )
                })
            });
            space = space
                .bind(Exact::new(format!("urn:file:{n}")), endpoint)
                .bind(Exact::new(format!("urn:summary:{n}")), composition);
        }
        let kernel = Kernel::new(Arc::new(space))
            .with_cache_policy(Arc::new(Lru::with_bound(CacheBound::new(8, bound))));
        let cap = Capability::root();
        for _ in 0..2 {
            for n in 0..3 {
                let answer =
                    block_on(kernel.issue(source(&format!("urn:summary:{n}")), &cap)).unwrap();
                assert_eq!(answer.bytes, b"small");
            }
        }
        assert_eq!(
            kernel.cache_len(),
            6,
            "three replay handles and three buffers"
        );
        for n in 0..3 {
            assert!(kernel.is_cached(&source(&format!("urn:file:{n}")), &cap));
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            3,
            "bound {bound}: second reads must hit"
        );
    }
}
