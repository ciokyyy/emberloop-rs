#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use emberloop_core::context::{Context, ContextError, Plugin};

#[derive(Clone, Debug, PartialEq)]
struct Config(&'static str);

#[derive(Clone, Debug, PartialEq)]
struct Log(Vec<&'static str>);

struct NeedsConfig {
    runs: Arc<AtomicUsize>,
    log: Arc<std::sync::Mutex<Vec<&'static str>>>,
    uses: Arc<AtomicUsize>,
}

impl Plugin for NeedsConfig {
    fn name(&self) -> &str {
        "needs-config"
    }

    fn deps(&self) -> Vec<TypeId> {
        vec![TypeId::of::<Config>()]
    }

    fn apply(&self, ctx: &Context) -> Result<(), ContextError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let cfg = ctx.require::<Config>()?;
        self.log.lock().unwrap().push(cfg.0);
        let uses = Arc::clone(&self.uses);
        ctx.on_dispose(move || {
            uses.fetch_add(1, Ordering::SeqCst);
        });
        Ok(())
    }
}

#[tokio::test]
async fn a_plugin_waits_for_its_dependency_and_follows_it_in_and_out() {
    let runs = Arc::new(AtomicUsize::new(0));
    let disposes = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ctx = Context::new();
    ctx.plugin(NeedsConfig {
        runs: Arc::clone(&runs),
        log: Arc::clone(&log),
        uses: Arc::clone(&disposes),
    });
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no dependency yet");

    ctx.provide(Config("first"));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(log.lock().unwrap().as_slice(), ["first"]);

    ctx.remove::<Config>();
    assert_eq!(disposes.load(Ordering::SeqCst), 1);

    ctx.provide(Config("second"));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(log.lock().unwrap().as_slice(), ["first", "second"]);
}

#[tokio::test]
async fn disposers_run_in_reverse_order_and_listeners_die_with_their_scope() {
    let ctx = Context::new();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::new(AtomicUsize::new(0));
    let scope = ctx.scope();
    {
        let order = Arc::clone(&order);
        scope.on_dispose(move || order.lock().unwrap().push("first"));
    }
    {
        let order = Arc::clone(&order);
        scope.on_dispose(move || order.lock().unwrap().push("second"));
    }
    let seen_by_listener = Arc::clone(&seen);
    let _listener = scope.on(emberloop_core::context::Mode::Emit, move |_: Arc<u32>| {
        let seen = Arc::clone(&seen_by_listener);
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            None
        }
    });
    ctx.emit(1u32).await;
    assert_eq!(seen.load(Ordering::SeqCst), 1);

    scope.dispose();
    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["second", "first"],
        "reverse registration order"
    );
    ctx.emit(2u32).await;
    assert_eq!(seen.load(Ordering::SeqCst), 1, "listener is gone");
}

#[tokio::test]
async fn bail_stops_at_the_first_answer_and_serial_keeps_order() {
    let ctx = Context::new();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut listeners = Vec::new();
    for value in [1u32, 2, 3] {
        let order = Arc::clone(&order);
        listeners.push(ctx.on(
            emberloop_core::context::Mode::Bail,
            move |event: Arc<u32>| {
                let order = Arc::clone(&order);
                async move {
                    order.lock().unwrap().push(*event);
                    (*event == value).then(|| serde_json::json!(value))
                }
            },
        ));
    }
    let answer = ctx.bail(2u32).await;
    assert_eq!(answer, Some(serde_json::json!(2)));
    assert_eq!(
        order.lock().unwrap().as_slice(),
        [2, 2],
        "third handler never ran"
    );

    let serial_order = Arc::new(std::sync::Mutex::new(Vec::new()));
    for tag in ["a", "b", "c"] {
        let order = Arc::clone(&serial_order);
        listeners.push(
            ctx.on(emberloop_core::context::Mode::Serial, move |_: Arc<()>| {
                let order = Arc::clone(&order);
                async move {
                    order.lock().unwrap().push(tag);
                    None
                }
            }),
        );
    }
    ctx.serial(()).await;
    assert_eq!(
        serial_order.lock().unwrap().as_slice(),
        ["a", "b", "c"],
        "serial keeps registration order"
    );
}

#[tokio::test]
async fn a_child_context_overrides_one_service_and_shares_the_rest() {
    let parent = Context::new();
    parent.provide(Config("parent"));
    parent.provide(Log(vec!["shared"]));

    let child = parent.child();
    child.provide(Config("child"));

    assert_eq!(*child.require::<Config>().unwrap(), Config("child"));
    assert_eq!(*parent.require::<Config>().unwrap(), Config("parent"));
    assert_eq!(*child.require::<Log>().unwrap(), Log(vec!["shared"]));
    child.remove::<Config>();
    assert_eq!(*child.require::<Config>().unwrap(), Config("parent"));
}

struct Exploding;

impl Plugin for Exploding {
    fn name(&self) -> &str {
        "exploding"
    }

    fn apply(&self, _ctx: &Context) -> Result<(), ContextError> {
        panic!("boom");
    }
}

#[tokio::test]
async fn a_panicking_plugin_leaves_the_tree_alive() {
    let ctx = Context::new();
    ctx.plugin(Exploding);
    ctx.provide(Config("still here"));
    assert_eq!(*ctx.require::<Config>().unwrap(), Config("still here"));
}

#[derive(Clone, Debug, PartialEq)]
struct Tokens(u32);

trait FakeHarness: Send + Sync {
    fn tokens(&self) -> Tokens;
}

struct FakeHarnessPlugin;

impl Plugin for FakeHarnessPlugin {
    fn name(&self) -> &str {
        "fake-harness"
    }

    fn apply(&self, ctx: &Context) -> Result<(), ContextError> {
        ctx.provide::<Arc<dyn FakeHarness>>(Arc::new(Zero));
        Ok(())
    }
}

struct Zero;

impl FakeHarness for Zero {
    fn tokens(&self) -> Tokens {
        Tokens(0)
    }
}

#[tokio::test]
async fn a_fake_harness_is_one_plugin_line() {
    let ctx = Context::new();
    ctx.plugin(FakeHarnessPlugin);
    let harness = ctx.require::<Arc<dyn FakeHarness>>().unwrap();
    assert_eq!(harness.tokens(), Tokens(0));
}

#[tokio::test]
async fn registries_collect_extension_points() {
    let ctx = Context::new();
    let registry = ctx.registry::<Arc<dyn FakeHarness>>();
    registry.register(Arc::new(Zero));
    assert_eq!(registry.list().len(), 1);
    let again = ctx.registry::<Arc<dyn FakeHarness>>();
    assert!(Arc::ptr_eq(&registry, &again));
    assert_eq!(again.list().len(), 1);
}
