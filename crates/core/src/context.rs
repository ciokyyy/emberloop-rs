//! Cordis-shaped context tree: typed services, plugins with dependencies, scoped
//! disposers, event modes and extension-point registries.
use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use tokio_util::sync::CancellationToken;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub type Value = serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Emit,
    Bail,
    Serial,
    Parallel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    Missing(&'static str),
    Panicked(String),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "service {name} is not provided"),
            Self::Panicked(name) => write!(f, "plugin {name} panicked while applying"),
        }
    }
}

impl std::error::Error for ContextError {}

pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn deps(&self) -> Vec<TypeId> {
        Vec::new()
    }
    fn apply(&self, ctx: &Context) -> Result<(), ContextError>;
}

pub struct Registry<T> {
    items: RwLock<Vec<T>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            items: RwLock::new(Vec::new()),
        }
    }
}

impl<T: Clone> Registry<T> {
    pub fn register(&self, item: T) {
        self.items
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(item);
    }

    pub fn list(&self) -> Vec<T> {
        self.items
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

type ErasedHandler =
    Arc<dyn Fn(Arc<dyn Any + Send + Sync>) -> BoxFuture<Option<Value>> + Send + Sync>;

struct ListenerEntry {
    owner: Weak<Inner>,
    id: u64,
    mode: Mode,
    handler: ErasedHandler,
}

struct PluginSlot {
    plugin: Arc<dyn Plugin>,
    owner: Weak<Inner>,
    scope: Option<Context>,
    failed: bool,
}

#[derive(Default)]
struct Shared {
    plugins: Mutex<Vec<PluginSlot>>,
    listeners: Mutex<HashMap<TypeId, Vec<ListenerEntry>>>,
}

#[derive(Default)]
struct Inner {
    parent: Option<Arc<Inner>>,
    services: Arc<RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>,
    disposers: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    shared: Arc<Shared>,
    cancel_token: CancellationToken,
    disposed: AtomicBool,
}

impl Inner {
    fn resolve(&self, key: TypeId) -> Option<Arc<dyn Any + Send + Sync>> {
        let local = self
            .services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned();
        local.or_else(|| self.parent.as_ref().and_then(|parent| parent.resolve(key)))
    }

    fn dispose(&self) {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let disposers = std::mem::take(
            &mut *self
                .disposers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for disposer in disposers.into_iter().rev() {
            disposer();
        }
        self.cancel_token.cancel();
    }
}

#[derive(Clone)]
pub struct Context {
    inner: Arc<Inner>,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    pub fn new() -> Self {
        let inner = Arc::new(Inner {
            services: Arc::new(RwLock::new(HashMap::new())),
            shared: Arc::new(Shared::default()),
            cancel_token: CancellationToken::new(),
            ..Inner::default()
        });
        Self { inner }
    }

    fn adopt(parent: &Context) -> Self {
        let inner = Arc::new(Inner {
            services: Arc::new(RwLock::new(HashMap::new())),
            parent: Some(Arc::clone(&parent.inner)),
            shared: Arc::clone(&parent.inner.shared),
            cancel_token: parent.inner.cancel_token.child_token(),
            ..Inner::default()
        });
        Self { inner }
    }

    pub fn child(&self) -> Self {
        Self::adopt(self)
    }

    pub fn scope(&self) -> Scope {
        Scope(Self::adopt(self))
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.cancel_token.clone()
    }

    pub fn cancel(&self) {
        self.inner.cancel_token.cancel();
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::SeqCst)
    }

    pub fn dispose(&self) {
        self.inner.dispose();
        let shared = Arc::clone(&self.inner.shared);
        let mut listeners = shared
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entries in listeners.values_mut() {
            entries.retain(|entry| {
                entry
                    .owner
                    .upgrade()
                    .is_some_and(|inner| !inner.disposed.load(Ordering::SeqCst))
            });
        }
    }

    pub fn on_dispose(&self, disposer: impl FnOnce() + Send + 'static) {
        self.inner
            .disposers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Box::new(disposer));
    }

    pub fn provide<T: Send + Sync + 'static>(&self, service: T) {
        self.provide_any(TypeId::of::<T>(), Arc::new(service));
        self.sync_plugins();
    }

    fn provide_any(&self, key: TypeId, service: Arc<dyn Any + Send + Sync>) {
        self.inner
            .services
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, service);
    }

    pub fn remove<T: Send + Sync + 'static>(&self) {
        self.inner
            .services
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&TypeId::of::<T>());
        self.sync_plugins();
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.inner
            .resolve(TypeId::of::<T>())
            .and_then(|service| service.downcast::<T>().ok())
    }

    pub fn require<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, ContextError> {
        self.get::<T>()
            .ok_or(ContextError::Missing(type_name::<T>()))
    }

    pub fn registry<T: Send + Sync + 'static>(&self) -> Arc<Registry<T>> {
        if let Ok(existing) = self.require::<Arc<Registry<T>>>() {
            return Arc::unwrap_or_clone(existing);
        }
        let registry = Arc::new(Registry::<T>::default());
        self.provide(Arc::clone(&registry));
        registry
    }

    pub fn plugin<P: Plugin>(&self, plugin: P) {
        self.inner
            .shared
            .plugins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(PluginSlot {
                plugin: Arc::new(plugin),
                owner: Arc::downgrade(&self.inner),
                scope: None,
                failed: false,
            });
        self.sync_plugins();
    }

    fn sync_plugins(&self) {
        let shared = Arc::clone(&self.inner.shared);
        let mut slots = std::mem::take(
            &mut *shared
                .plugins
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for slot in &mut slots {
            let deps_met = slot.plugin.deps().iter().all(|key| {
                slot.owner
                    .upgrade()
                    .is_some_and(|owner| owner.resolve(*key).is_some())
            });
            match (&slot.scope, deps_met) {
                (Some(scope), false) => {
                    scope.dispose();
                    slot.scope = None;
                }
                (None, true) if !slot.failed => {
                    let Some(owner) = slot.owner.upgrade() else {
                        continue;
                    };
                    let scope = Context {
                        inner: Arc::new(Inner {
                            services: Arc::clone(&owner.services),
                            parent: Some(owner),
                            shared: Arc::clone(&shared),
                            cancel_token: CancellationToken::new(),
                            ..Inner::default()
                        }),
                    };
                    slot.scope = Some(scope.clone());
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        slot.plugin.apply(&scope)
                    }))
                    .is_err()
                    {
                        slot.failed = true;
                        scope.dispose();
                        slot.scope = None;
                    }
                }
                _ => {}
            }
        }
        slots.retain(|slot| slot.owner.strong_count() > 0);
        shared
            .plugins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(slots);
    }

    pub fn on<E, F, Fut>(&self, mode: Mode, handler: F) -> Listener
    where
        E: Send + Sync + 'static,
        F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Value>> + Send + 'static,
    {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let source = Arc::new(handler);
        let erased: ErasedHandler = Arc::new(move |event: Arc<dyn Any + Send + Sync>| {
            let Ok(event) = event.downcast::<E>() else {
                return Box::pin(async { None }) as BoxFuture<Option<Value>>;
            };
            let source = Arc::clone(&source);
            Box::pin(async move { source(event).await })
        });
        self.inner
            .shared
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(TypeId::of::<E>())
            .or_default()
            .push(ListenerEntry {
                owner: Arc::downgrade(&self.inner),
                id,
                mode,
                handler: erased,
            });
        Listener {
            shared: Arc::clone(&self.inner.shared),
            event: TypeId::of::<E>(),
            id,
        }
    }

    fn entries<E: 'static>(&self, mode: Mode) -> Vec<ErasedHandler> {
        let shared = &self.inner.shared;
        let mut listeners = shared
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entries) = listeners.get_mut(&TypeId::of::<E>()) else {
            return Vec::new();
        };
        entries.retain(|entry| {
            entry
                .owner
                .upgrade()
                .is_some_and(|inner| !inner.disposed.load(Ordering::SeqCst))
        });
        entries
            .iter()
            .filter(|entry| entry.mode == mode)
            .map(|entry| Arc::clone(&entry.handler))
            .collect()
    }

    pub async fn emit<E: Send + Sync + 'static>(&self, event: E) {
        let event: Arc<dyn Any + Send + Sync> = Arc::new(event);
        for handler in self.entries::<E>(Mode::Emit) {
            let _ = handler(Arc::clone(&event)).await;
        }
    }

    pub async fn bail<E: Send + Sync + 'static>(&self, event: E) -> Option<Value> {
        let event: Arc<dyn Any + Send + Sync> = Arc::new(event);
        for handler in self.entries::<E>(Mode::Bail) {
            if let Some(answer) = handler(Arc::clone(&event)).await {
                return Some(answer);
            }
        }
        None
    }

    pub async fn serial<E: Send + Sync + 'static>(&self, event: E) -> Vec<Option<Value>> {
        let event: Arc<dyn Any + Send + Sync> = Arc::new(event);
        let mut answers = Vec::new();
        for handler in self.entries::<E>(Mode::Serial) {
            answers.push(handler(Arc::clone(&event)).await);
        }
        answers
    }

    pub async fn parallel<E: Send + Sync + 'static>(&self, event: E) -> Vec<Option<Value>> {
        let event: Arc<dyn Any + Send + Sync> = Arc::new(event);
        let mut tasks = tokio::task::JoinSet::new();
        for handler in self.entries::<E>(Mode::Parallel) {
            let event = Arc::clone(&event);
            tasks.spawn(async move { handler(event).await });
        }
        let mut answers = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            answers.push(joined.unwrap_or(None));
        }
        answers
    }
}

pub struct Listener {
    shared: Arc<Shared>,
    event: TypeId,
    id: u64,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let mut listeners = self
            .shared
            .listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entries) = listeners.get_mut(&self.event) {
            entries.retain(|entry| entry.id != self.id);
        }
    }
}

impl std::ops::Deref for Scope {
    type Target = Context;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct Scope(Context);

impl Scope {
    pub fn context(&self) -> &Context {
        &self.0
    }

    pub fn dispose(self) {
        self.0.dispose();
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.0.dispose();
    }
}
