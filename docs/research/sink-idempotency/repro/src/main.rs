//! Reproduces the duplicate a retried `Sink` leaves when a transport times out AFTER the
//! write landed, against the published modules themselves, and the gaps the existing keys
//! leave. Every finding is asserted, so this program fails when a module changes the
//! behavior it reports; `docs/research/sink-idempotency.md` records its output.
//!
//! The fake transport is [`LossyLink`]: it delivers the request to a real kernel, so the
//! write happens, then loses the answer and reports `Error::Timeout`, which is what a door,
//! an IPC socket or a QUIC stream does when the connection drops between the write and the
//! reply. The caller is [`agent_send`]: the loop an agent runs by default, which sends the
//! same request again on a transient failure. [`contract_send`] is the policy
//! `ikigai-throttle`'s `Retry` follows (ledger #3381): send again only when the Sink's
//! contract declares a resend safe, and, under a key, only when the call carries it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Clock, Error, Fallback, Iri, Kernel, Representation, Request, Resolution,
    Scope, Space, Time, Verb,
};
use ikigai_store::DurableStore;

// ------------------------------------------------------------------------ the transport

/// A link that delivers every request, and loses the answer of one when told to.
struct LossyLink {
    kernel: Kernel,
    lose: AtomicBool,
    /// The answers the link lost, byte for byte: what the first attempt really said.
    lost: Mutex<Vec<Vec<u8>>>,
}

impl LossyLink {
    fn new(kernel: Kernel) -> Self {
        LossyLink {
            kernel,
            lose: AtomicBool::new(false),
            lost: Mutex::new(Vec::new()),
        }
    }

    /// Lose the answer of the next send (the write still happens).
    fn lose_next(&self) {
        self.lose.store(true, Ordering::SeqCst);
    }

    fn send(&self, request: Request, cap: &Capability) -> Result<Representation, Error> {
        let answer = block_on(self.kernel.issue(request, cap));
        if self.lose.swap(false, Ordering::SeqCst) {
            if let Ok(repr) = &answer {
                self.lost.lock().unwrap().push(repr.bytes.clone());
            }
            return Err(Error::Timeout(
                "the link dropped after the request was delivered".into(),
            ));
        }
        answer
    }

    fn last_lost(&self) -> String {
        let lost = self.lost.lock().unwrap();
        String::from_utf8_lossy(lost.last().expect("a lost answer")).into_owned()
    }
}

/// An agent's default: on a transient failure, send the same request again (three tries).
fn agent_send(
    link: &LossyLink,
    request: &Request,
    cap: &Capability,
) -> (Result<String, Error>, usize) {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match link.send(request.clone(), cap) {
            Err(e) if e.is_transient() && attempts < 3 => continue,
            other => return (other.map(|r| text(&r)), attempts),
        }
    }
}

/// The contract-respecting retry: what `ikigai-throttle`'s `Retry` re-sends, and no more.
fn contract_send(
    link: &LossyLink,
    space: &dyn Space,
    request: &Request,
    cap: &Capability,
) -> (Result<String, Error>, usize) {
    let safe = resend_declared_safe(space, request);
    let mut attempts = 0;
    loop {
        attempts += 1;
        match link.send(request.clone(), cap) {
            Err(e) if e.is_transient() && safe && attempts < 3 => continue,
            other => return (other.map(|r| text(&r)), attempts),
        }
    }
}

/// Whether the endpoint's contract says THIS call may be sent again (`ActionSpec::resend`).
fn resend_declared_safe(space: &dyn Space, request: &Request) -> bool {
    let meta = Request::new(Verb::Meta, request.target.clone());
    let Resolution::Hit(resolved) = space.resolve(&meta, &Scope::empty()) else {
        panic!("unresolved {}", request.target)
    };
    resolved
        .endpoint
        .describe()
        .action_specs()
        .into_iter()
        .find(|a| a.verb == request.verb)
        .is_some_and(|a| a.resend_is_safe(|name| request.args.contains_key(name)))
}

// --------------------------------------------------------------------------- fixtures

/// An advancing clock: the ledger and the layers refuse to write without one.
struct TickingClock(AtomicU64);

impl Clock for TickingClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.fetch_add(1_000, Ordering::SeqCst))
    }
}

fn clock() -> Arc<TickingClock> {
    Arc::new(TickingClock(AtomicU64::new(1_795_000_000_000)))
}

fn request(verb: Verb, iri: &str, args: &[(&str, &str)]) -> Request {
    args.iter().fold(
        Request::new(verb, Iri::parse(iri).expect("an IRI")),
        |r, (name, value)| r.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    )
}

fn text(repr: &Representation) -> String {
    String::from_utf8_lossy(&repr.bytes).into_owned()
}

fn read(link: &LossyLink, cap: &Capability, iri: &str, args: &[(&str, &str)]) -> String {
    link.send(request(Verb::Source, iri, args), cap)
        .map(|r| text(&r))
        .unwrap_or_else(|e| panic!("Source {iri}: {e}"))
}

const PARTICIPANTS: &str = "urn:collab:repro:participants";

fn ledger_space() -> Arc<Fallback> {
    let ledger =
        ikigai_ledger::space_with(ikigai_ledger::SpaceConfig::default().participants(PARTICIPANTS));
    Arc::new(Fallback::new(vec![
        Arc::new(ikigai_store::space(DurableStore::in_memory().unwrap())) as Arc<dyn Space>,
        Arc::new(ledger) as Arc<dyn Space>,
    ]))
}

fn ledger_link() -> (LossyLink, Arc<Fallback>) {
    let space = ledger_space();
    let kernel = Kernel::with_meta_renderer(space.clone(), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(clock());
    (LossyLink::new(kernel), space)
}

/// A writer of the default ledger, as a door mints the grant for one principal.
fn writer(principal: &str) -> Capability {
    Capability::scoped(vec![
        "urn:cap:ledger:read:default".to_string(),
        "urn:cap:ledger:write:default".to_string(),
        "urn:cap:store:read:graph:urn:iki:ledger:graph:default".to_string(),
        "urn:cap:store:write:graph:urn:iki:ledger:graph:default".to_string(),
        format!("urn:cap:store:read:graph:{PARTICIPANTS}"),
    ])
    .with_principal(principal)
    .unwrap()
}

fn item_count(link: &LossyLink) -> String {
    read(
        link,
        &Capability::root(),
        "urn:iki:ledger:count",
        &[("status", "all")],
    )
    .trim()
    .to_string()
}

// ------------------------------------------------------------------------- the report

struct Report(Vec<(String, String)>);

impl Report {
    fn row(&mut self, case: &str, observed: impl Into<String>) {
        let observed = observed.into();
        println!("- {case}\n    {}", observed.replace('\n', "\n    "));
        self.0.push((case.into(), observed));
    }
}

fn main() {
    let mut report = Report(Vec::new());
    println!("Sink resends over a link that loses the answer after the write lands\n");
    ledger_append(&mut report);
    ledger_append_request_token(&mut report);
    ledger_keyed_append(&mut report);
    ledger_key_scope(&mut report);
    ledger_comment(&mut report);
    ledger_state(&mut report);
    layers_say(&mut report);
    println!("\n{} findings reproduced.", report.0.len());
}

/// 1. An append with no key: the retry files a second item.
fn ledger_append(report: &mut Report) {
    let (link, space) = ledger_link();
    let cap = Capability::root();
    let append = request(
        Verb::Sink,
        "urn:iki:ledger:append",
        &[("content", "Port the importer")],
    );

    link.lose_next();
    let (answer, attempts) = agent_send(&link, &append, &cap);
    let answer = answer.expect("the retry succeeds");
    assert_eq!(attempts, 2);
    assert!(answer.starts_with("#2 "), "{answer}");
    assert_eq!(item_count(&link), "2");
    report.row(
        "ledger append, no key, agent retry",
        format!(
            "lost answer: {}\nretry answer: {}\nitems now: {} (one intended)",
            link.last_lost().trim(),
            answer.trim(),
            item_count(&link)
        ),
    );

    // The contract-respecting retry does not duplicate it, and leaves the caller blind.
    let (link, _) = ledger_link();
    link.lose_next();
    let (answer, attempts) = contract_send(&link, &*space, &append, &cap);
    assert_eq!(attempts, 1);
    assert!(matches!(answer, Err(Error::Timeout(_))), "{answer:?}");
    assert_eq!(item_count(&link), "1");
    report.row(
        "ledger append, no key, contract retry",
        format!(
            "not re-sent (resend undeclared without key=); caller sees: {}; items now: 1, \
             which the caller cannot learn",
            answer.unwrap_err()
        ),
    );
}

/// The published ledger does not yet implement append request tokens (ledger #4057).
/// Flip this case to one item and identical answers when the release carrying it is used.
fn ledger_append_request_token(report: &mut Report) {
    let (link, space) = ledger_link();
    let cap = Capability::root();
    let append = request(
        Verb::Sink,
        "urn:iki:ledger:append",
        &[
            ("content", "Port the importer"),
            ("idempotency-key", "repro-append-1"),
        ],
    );
    assert!(!resend_declared_safe(&*space, &append));
    link.lose_next();
    let (answer, attempts) = agent_send(&link, &append, &cap);
    let answer = answer.expect("the retry succeeds");
    assert_eq!(attempts, 2);
    assert_eq!(item_count(&link), "2");
    assert_ne!(link.last_lost(), answer);
    report.row(
        "ledger append idempotency-key=, agent retry (pending release)",
        format!(
            "lost answer: {}\nretry answer: {}\nitems now: 2 (one intended)",
            link.last_lost().trim(),
            answer.trim()
        ),
    );
}

/// 2. A keyed append: no duplicate, but the retry's answer is not the first answer, the key
///    matches whatever content comes with it, and a closed item still answers `existing`.
fn ledger_keyed_append(report: &mut Report) {
    let (link, space) = ledger_link();
    let cap = Capability::root();
    let keyed = request(
        Verb::Sink,
        "urn:iki:ledger:append",
        &[
            ("content", "Port the importer"),
            ("key", "urn:repro:task:1"),
        ],
    );
    assert!(resend_declared_safe(&*space, &keyed));

    link.lose_next();
    let (answer, attempts) = contract_send(&link, &*space, &keyed, &cap);
    let answer = answer.expect("the keyed retry succeeds");
    assert_eq!(attempts, 2);
    assert_eq!(item_count(&link), "1");
    let first = link.last_lost();
    assert_ne!(
        first, answer,
        "the replay answers differently from the first filing"
    );
    assert!(answer.trim_end().ends_with("existing open"), "{answer}");
    report.row(
        "ledger append key=, contract retry",
        format!(
            "items now: 1 (no duplicate)\nfirst answer:  {}\nretry answer:  {}\n\
             not the first answer byte for byte",
            first.trim(),
            answer.trim()
        ),
    );

    // The same key with different content: `existing`, and the content is dropped silently.
    let other = request(
        Verb::Sink,
        "urn:iki:ledger:append",
        &[
            ("content", "Something else entirely"),
            ("key", "urn:repro:task:1"),
        ],
    );
    let answer = link.send(other, &cap).map(|r| text(&r)).unwrap();
    assert!(answer.trim_end().ends_with("existing open"), "{answer}");
    let item = read(&link, &cap, "urn:iki:ledger:item:1", &[]);
    assert!(!item.contains("Something else"), "{item}");
    report.row(
        "ledger append, same key, different content",
        format!(
            "answer: {}\nitem #1 keeps its first title; no fingerprint is compared",
            answer.trim()
        ),
    );

    // A closed item: a fan-out that reuses fixed keys files onto last wave's closed item.
    link.send(
        request(
            Verb::Sink,
            "urn:iki:ledger:close",
            &[("item", "#1"), ("reason", "done")],
        ),
        &cap,
    )
    .expect("close");
    let next_wave = request(
        Verb::Sink,
        "urn:iki:ledger:append",
        &[
            ("content", "Port the importer, second wave"),
            ("key", "urn:repro:task:1"),
        ],
    );
    let answer = link.send(next_wave, &cap).map(|r| text(&r)).unwrap();
    assert!(answer.trim_end().ends_with("existing closed"), "{answer}");
    report.row(
        "ledger append, same key after close",
        format!("answer: {} (the key is taken forever)", answer.trim()),
    );
}

/// 3. Keys are unique per LEDGER, not per principal: one writer's key answers another's
///    append with the first writer's item.
fn ledger_key_scope(report: &mut Report) {
    let (link, _) = ledger_link();
    let a = writer("urn:collab:participant:writer-a");
    let b = writer("urn:collab:participant:writer-b");
    let mine = link
        .send(
            request(
                Verb::Sink,
                "urn:iki:ledger:append",
                &[("content", "Writer A's task"), ("key", "task-1")],
            ),
            &a,
        )
        .map(|r| text(&r))
        .expect("writer a files");
    let theirs = link
        .send(
            request(
                Verb::Sink,
                "urn:iki:ledger:append",
                &[("content", "Writer B's unrelated task"), ("key", "task-1")],
            ),
            &b,
        )
        .map(|r| text(&r))
        .expect("writer b's append answers");
    assert!(theirs.trim_end().ends_with("existing open"), "{theirs}");
    assert_eq!(item_count(&link), "1");
    report.row(
        "ledger append, same key, two principals",
        format!(
            "writer a: {}\nwriter b: {}\nb's item is never filed and b is handed a's item",
            mine.trim(),
            theirs.trim()
        ),
    );
}

/// 4. A comment with no key: the retry posts it twice. With `key=` it does not.
fn ledger_comment(report: &mut Report) {
    let (link, space) = ledger_link();
    let cap = Capability::root();
    link.send(
        request(
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "An item")],
        ),
        &cap,
    )
    .expect("append");
    let comment = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[
            ("item", "#1"),
            ("content", "repro-note-7f3a"),
            ("author", "bot"),
        ],
    );
    assert!(!resend_declared_safe(&*space, &comment));
    link.lose_next();
    let (answer, attempts) = agent_send(&link, &comment, &cap);
    answer.expect("the retry succeeds");
    assert_eq!(attempts, 2);
    let item = read(&link, &cap, "urn:iki:ledger:item:1", &[]);
    let copies = item.matches("repro-note-7f3a").count();
    assert_eq!(copies, 2, "{item}");
    report.row(
        "ledger comment, no key, agent retry",
        format!("the comment appears {copies} times on #1"),
    );

    let keyed = request(
        Verb::Sink,
        "urn:iki:ledger:comment",
        &[
            ("item", "#1"),
            ("content", "repro-keyed-91c2"),
            ("key", "urn:repro:note:1"),
        ],
    );
    link.lose_next();
    let (answer, attempts) = contract_send(&link, &*space, &keyed, &cap);
    let answer = answer.expect("keyed retry");
    assert_eq!(attempts, 2);
    let item = read(&link, &cap, "urn:iki:ledger:item:1", &[]);
    assert_eq!(item.matches("repro-keyed-91c2").count(), 1, "{item}");
    let first = link.last_lost();
    report.row(
        "ledger comment key=, contract retry",
        format!(
            "one comment (no duplicate)\nfirst answer:  {}\nretry answer:  {}",
            first.trim(),
            answer.trim()
        ),
    );
}

/// 5. A state transition (compare-and-set) whose answer was lost: the retry is a Conflict
///    for work that succeeded.
fn ledger_state(report: &mut Report) {
    let (link, space) = ledger_link();
    let cap = Capability::root();
    link.send(
        request(
            Verb::Sink,
            "urn:iki:ledger:append",
            &[("content", "An item")],
        ),
        &cap,
    )
    .expect("append");
    let mv = request(
        Verb::Sink,
        "urn:iki:ledger:item:1:state",
        &[("from", "filed"), ("to", "queued")],
    );
    assert!(!resend_declared_safe(&*space, &mv));
    link.lose_next();
    let (answer, attempts) = agent_send(&link, &mv, &cap);
    assert_eq!(attempts, 2);
    let err = answer.expect_err("the retry is refused");
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    let state = read(&link, &cap, "urn:iki:ledger:item:1:state", &[]);
    assert_eq!(state.trim(), "queued");
    report.row(
        "ledger item state from=filed to=queued, agent retry",
        format!(
            "lost answer: {}\nretry answer: {}\nstate now: {} (the first attempt succeeded)",
            link.last_lost().trim(),
            first_line(&err.to_string()),
            state.trim()
        ),
    );
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

// ------------------------------------------------------------------------------ layers

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "sink-idempotency-repro-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        set_mode(&dir, 0o700);
        Scratch(dir)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

const SHAPES: &str = r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ik: <https://ikigai-rs.dev/ns#> .
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
<urn:repro:shape:message> a sh:NodeShape ;
  sh:targetClass ik:Message ;
  sh:closed true ;
  sh:ignoredProperties ( rdf:type ) ;
  sh:property [ sh:path ik:body ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ,
              [ sh:path prov:wasAttributedTo ; sh:minCount 1 ; sh:maxCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dcterms:created ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:dateTime ] ,
              [ sh:path ik:seq ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:integer ] .
"#;

const CONTEXT: &str = r#"{ "@context": {
  "ik": "https://ikigai-rs.dev/ns#",
  "Message": "ik:Message",
  "body": "ik:body"
} }
"#;

/// 6. A layer `say` (append-only): the retry leaves a permanent second entry.
fn layers_say(report: &mut Report) {
    let config = Scratch::new("config");
    let state = Scratch::new("state");
    config.write("layers/msg.shapes.ttl", SHAPES);
    config.write("layers/msg.context.jsonld", CONTEXT);
    config.write(
        "layers/talk.toml",
        "model = \"channel\"\nshapes = \"msg.shapes.ttl\"\ncontext = \"msg.context.jsonld\"\n",
    );
    config.write(
        "participants.ttl",
        "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n\
         <urn:collab:participant:writer-a> a foaf:Person .\n",
    );
    config.write(
        "collabs/repro.toml",
        "[[mount]]\nlayer = \"talk\"\nwriter-a = [\"read\", \"feed\", \"say\"]\n",
    );
    config.write(
        "enrollment.toml",
        "writer-a = \"aa00000000000000000000000000000000000000000000000000000000000001\"\n",
    );
    let key_path = config.0.join("state.key");
    {
        use age::secrecy::ExposeSecret;
        let identity = age::x25519::Identity::generate();
        std::fs::write(
            &key_path,
            format!("# repro\n{}\n", identity.to_string().expose_secret()),
        )
        .unwrap();
        set_mode(&key_path, 0o600);
    }
    let key = ikigai_layers::StateKey::load(&key_path).expect("a state key");
    let host = ikigai_layers::Host::open(&config.0, &state.0, &key).expect("a layer host");
    let space = Arc::new(Fallback::new(vec![
        Arc::new(ikigai_layers::space(&host)) as Arc<dyn Space>
    ]));
    let kernel = Kernel::with_meta_renderer(space.clone(), Arc::new(ikigai_vocab::TurtleRenderer))
        .with_clock(clock());
    let link = LossyLink::new(kernel);
    let writer =
        Capability::scoped(["read", "feed", "say"].map(|v| format!("urn:cap:layer:{v}:talk")))
            .with_principal("urn:collab:participant:writer-a")
            .unwrap();

    let say = request(
        Verb::Sink,
        "urn:layer:talk:say",
        &[(
            "content",
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n<urn:layer:new> a ik:Message ; ik:body \"repro-say-55d1\" .\n",
        )],
    );
    let declared = resend_declared_safe(&*space, &say);
    assert!(!declared);
    link.lose_next();
    let (answer, attempts) = agent_send(&link, &say, &writer);
    let answer = answer.expect("the retry succeeds");
    assert_eq!(attempts, 2);
    let first = link.last_lost();
    assert_ne!(first.trim(), answer.trim());
    let entries = read(&link, &writer, "urn:layer:talk:entries", &[]);
    let copies = entries.matches("repro-say-55d1").count();
    assert_eq!(copies, 2, "{entries}");
    report.row(
        "layers say, agent retry",
        format!(
            "first entry:  {}\nsecond entry: {}\nthe body appears {copies} times in the layer, \
             and a layer is append-only; say takes no key and declares no resend",
            first.trim(),
            answer.trim()
        ),
    );
}
