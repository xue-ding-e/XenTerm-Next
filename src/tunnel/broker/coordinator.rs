//! Trusted in-process coordinator only. There is no OS peer bootstrap or
//! consistent platform loader here; wire input cannot construct authority.
//! A future loader must pin the real profile and revalidate key/trust inputs
//! before supplying a snapshot and advancing/revoking its authorization cycle.
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}};
use anyhow::{anyhow, ensure, Result};
use futures::FutureExt;
use tokio::sync::watch;
use tokio::time::Instant;
use super::protocol::Error;
use super::registry::{Acquisition, AdapterMode, Connection, Registry, RouteKey, TrustedAdapterRegistration};
use super::runtime::ManagedForward;
use super::snapshot::ResolvedRouteSnapshot;

type StartupJoin = futures::future::Shared<futures::future::BoxFuture<'static, bool>>;
#[derive(Clone, Copy)]
enum Startup { Pending, Ready(SocketAddr), Failed, CleanupUnconfirmed }
#[derive(Default)]
struct Control {
    cancelled: bool,
    stop: Option<watch::Sender<bool>>,
    #[cfg(test)] bound: Option<SocketAddr>,
}
struct Resource {
    control: Mutex<Control>,
    runtime: tokio::sync::Mutex<Option<ManagedForward>>,
    drain_lock: tokio::sync::Mutex<()>,
    startup: Mutex<Option<StartupJoin>>,
    state: watch::Sender<Startup>,
    cleanup_confirmed: AtomicBool,
}
impl Resource {
    fn cancel(&self) {
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        control.cancelled = true;
        if let Some(stop) = &control.stop { stop.send_replace(true); }
    }
}
#[derive(Clone)]
enum Disconnection { Pending(Vec<(String, Arc<Resource>)>), Complete }
struct Core {
    registry: Registry,
    resources: HashMap<String, Arc<Resource>>,
    completed: VecDeque<(String, StartupJoin)>,
    disconnections: HashMap<Connection, Disconnection>,
}
impl Core {
    fn retire_disconnections(&mut self) {
        for state in self.disconnections.values_mut() {
            let complete = match state {
                Disconnection::Pending(resources) => resources.iter().all(|(_, resource)| {
                    if !resource.cleanup_confirmed.load(Ordering::Acquire) { return false; }
                    let joined = resource.startup.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    joined.and_then(|task| task.now_or_never()) == Some(true)
                }),
                Disconnection::Complete => false,
            };
            if complete { *state = Disconnection::Complete; }
        }
    }
    fn confirmed_drained(&mut self, listener: &str, joined: StartupJoin) -> std::result::Result<(), Error> {
        let resource = self.resources.get(listener).cloned().ok_or(Error::NotFound)?;
        self.registry.drained(listener)?;
        self.resources.remove(listener);
        resource.cleanup_confirmed.store(true, Ordering::Release);
        self.completed.push_back((listener.to_owned(), joined));
        if self.completed.len() > 128 { self.completed.pop_front(); }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct ActivationBarrier {
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
    pub prepared: Mutex<Option<SocketAddr>>,
}

pub(super) struct Coordinator {
    core: Arc<Mutex<Core>>,
    #[cfg(test)]
    pub before_activation: Option<Arc<ActivationBarrier>>,
    #[cfg(test)]
    pub before_drain: Option<Arc<ActivationBarrier>>,
    #[cfg(test)]
    pub after_runtime_drain: Option<Arc<ActivationBarrier>>,
}

/// Lease belongs to the verified connection, not to this reply future. Lost
/// replies are recovered through the request ledger/snapshot; dropping a waiter
/// must not secretly revoke another reference or manufacture a new lease.
pub(super) struct Reservation {
    pub acquisition: Acquisition,
    resource: Arc<Resource>,
    core: Weak<Mutex<Core>>,
    connection: Connection,
}
impl Reservation {
    #[cfg(test)] pub fn bound_address(&self) -> Option<SocketAddr> { self.resource.control.lock().unwrap().bound }

    #[cfg(test)]
    pub async fn observe_completion(&self) -> Result<()> {
        // Read-only test observer: never sends stop, release or drain.
        let joined = self.resource.startup.lock().unwrap().clone().unwrap();
        ensure!(joined.await, "lifecycle task failed");
        let core = self.core.upgrade().ok_or_else(|| anyhow!("coordinator unavailable"))?;
        let core = core.lock().unwrap();
        ensure!(!core.resources.contains_key(&self.acquisition.listener), "resource still stopping");
        ensure!(core.completed.iter().any(|(id, _)| id == &self.acquisition.listener), "completion missing");
        Ok(())
    }
    pub async fn ready(&self) -> Result<SocketAddr> {
        let mut state = self.resource.state.subscribe();
        loop {
            let value = *state.borrow_and_update();
            match value {
                Startup::Pending => state.changed().await.map_err(|_| anyhow!("startup unavailable"))?,
                Startup::Failed => return Err(anyhow!("saved dynamic startup failed")),
                Startup::CleanupUnconfirmed => return Err(anyhow!("cleanup_unconfirmed: resource remains stopping")),
                Startup::Ready(address) => {
                    let core = self.core.upgrade().ok_or_else(|| anyhow!("coordinator closed"))?;
                    let core = core.lock().unwrap_or_else(|e| e.into_inner());
                    ensure!(core.registry.live(&self.acquisition.listener), "dynamic authorization changed");
                    let leases = core.registry.snapshot(self.connection).map_err(|_| anyhow!("connection unavailable"))?;
                    ensure!(leases.iter().any(|(lease, _)| lease == &self.acquisition.lease), "dynamic lease released");
                    return Ok(address);
                }
            }
        }
    }
}
impl Coordinator {
    pub fn new() -> Self {
        Self { core: Arc::new(Mutex::new(Core { registry: Registry::new(), resources: HashMap::new(), completed: VecDeque::new(), disconnections: HashMap::new() })),
            #[cfg(test)] before_activation: None, #[cfg(test)] before_drain: None, #[cfg(test)] after_runtime_drain: None }
    }
    /// Internal saved-authority bootstrap, not an IPC operation. A caller must
    /// already possess a trusted loader decision; no externally chosen policy,
    /// generation, profile, HTTP principal or deadline may enter this method.
    pub fn authorize_saved(&self, policy: [u8; 32], generation: u64) -> std::result::Result<(), Error> {
        self.core.lock().unwrap_or_else(|e| e.into_inner()).registry.authorize_domain(policy, generation)
    }
    pub fn connect_saved(&self, policy: [u8; 32], generation: u64) -> std::result::Result<Connection, Error> {
        self.core.lock().unwrap_or_else(|e| e.into_inner()).registry.connect(TrustedAdapterRegistration {
            mode: AdapterMode::LocalSaved, saved_policy: policy, authorization_generation: generation,
        })
    }
    pub fn epoch(&self) -> String {
        self.core.lock().unwrap_or_else(|e| e.into_inner()).registry.epoch().to_owned()
    }
    pub fn reserve(&self, connection: Connection, epoch: &str, snapshot: Arc<ResolvedRouteSnapshot>,
        ensure: bool) -> Result<Reservation> {
        let bind = snapshot.bind_address()?;
        let route = RouteKey { session_id: snapshot.session_id().to_owned(), forward_index: snapshot.rule_index(),
            revision: snapshot.fingerprint(), permission_generation: snapshot.permission_generation() };
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let acquisition = core.registry.acquire(connection, epoch, route, bind, ensure)
            .map_err(|error| anyhow!("dynamic reservation rejected: {error:?}"))?;
        let resource = if acquisition.start {
            let resource = Arc::new(Resource { control: Mutex::new(Control::default()),
                runtime: tokio::sync::Mutex::new(None), drain_lock: tokio::sync::Mutex::new(()),
                startup: Mutex::new(None), state: watch::channel(Startup::Pending).0, cleanup_confirmed: AtomicBool::new(false) });
            core.resources.insert(acquisition.listener.clone(), resource.clone());
            let owner = Arc::downgrade(&self.core);
            let listener = acquisition.listener.clone();
            let owned_resource = resource.clone();
            #[cfg(test)] let barrier = self.before_activation.clone();
            #[cfg(test)] let drain_barrier = self.before_drain.clone();
            #[cfg(test)] let joined_barrier = self.after_runtime_drain.clone();
            // Registration occurs under the same core lock as reserve/release.
            // Even an immediately cancelled startup remains owned and joinable.
            let worker = tokio::spawn(async move {
                let checked_owner = owner.clone(); let checked_listener = listener.clone();
                let authorized = Arc::new(move || checked_owner.upgrade().is_some_and(|core|
                    core.lock().unwrap_or_else(|e| e.into_inner()).registry.live(&checked_listener)));
                let mut runtime = match ManagedForward::prepare(snapshot.clone(), authorized).await {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        owned_resource.state.send_replace(Startup::Failed);
                        if let Some(core) = owner.upgrade() {
                            let mut core = core.lock().unwrap_or_else(|e| e.into_inner());
                            let _ = core.registry.stop(&listener);
                        }
                        return true;
                    }
                };
                {
                    let mut control = owned_resource.control.lock().unwrap_or_else(|e| e.into_inner());
                    control.stop = Some(runtime.stop_handle());
                    #[cfg(test)] { control.bound = Some(runtime.bound_address()); }
                    if control.cancelled { control.stop.as_ref().unwrap().send_replace(true); }
                }
                let prepared = runtime.ready().await;
                #[cfg(test)] if prepared.is_ok() {
                    if let Some(barrier) = barrier {
                        *barrier.prepared.lock().unwrap() = prepared.as_ref().ok().copied();
                        barrier.entered.notify_one(); barrier.release.notified().await;
                    }
                }
                let activated = prepared.ok().and_then(|address| {
                    let core = owner.upgrade()?;
                    let mut core = core.lock().unwrap_or_else(|e| e.into_inner());
                    // Authorization-cycle validation and ready transition are
                    // indivisible relative to revoke/release/config changes.
                    core.registry.ready(&listener, snapshot.permission_generation()).ok()?;
                    if runtime.activate().is_err() {
                        let _ = core.registry.stop(&listener);
                        return None;
                    }
                    Some(address)
                });
                if let Some(address) = activated {
                    owned_resource.state.send_replace(Startup::Ready(address));
                    let mut finished = runtime.finished();
                    if !*finished.borrow() { let _ = finished.changed().await; }
                }
                // Failed startup, revoked activation, or a disconnected route
                // all use the same owned drain before releasing the bind.
                owned_resource.cancel();
                if let Some(core) = owner.upgrade() {
                    let _ = core.lock().unwrap_or_else(|e| e.into_inner()).registry.stop(&listener);
                }
                #[cfg(test)] if let Some(barrier) = drain_barrier {
                    barrier.entered.notify_one(); barrier.release.notified().await;
                }
                let cleanup = runtime.drain_until(Instant::now() + std::time::Duration::from_secs(5)).await;
                if cleanup.is_err() {
                    *owned_resource.runtime.lock().await = Some(runtime);
                    owned_resource.state.send_replace(Startup::CleanupUnconfirmed);
                }
                #[cfg(test)] if cleanup.is_ok() {
                    if let Some(barrier) = joined_barrier {
                        barrier.entered.notify_one(); barrier.release.notified().await;
                    }
                }
                cleanup.is_ok()
            });
            let supervised_resource = resource.clone();
            let supervised_owner = Arc::downgrade(&self.core);
            let supervised_listener = acquisition.listener.clone();
            let supervisor = tokio::spawn(async move {
                let outcome = worker.await;
                let joined = outcome.is_ok();
                if matches!(outcome, Ok(true)) {
                    // Physical cleanup alone is insufficient: publish terminal
                    // ownership only after the lifecycle worker actually joined.
                    if let Some(core) = supervised_owner.upgrade() {
                        let mut core = core.lock().unwrap_or_else(|e| e.into_inner());
                        let completion = supervised_resource.startup.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        if let Some(completion) = completion {
                            let _ = core.confirmed_drained(&supervised_listener, completion);
                        }
                    }
                    supervised_resource.state.send_replace(Startup::Failed);
                } else {
                    supervised_resource.cancel();
                    supervised_resource.state.send_replace(Startup::CleanupUnconfirmed);
                    if let Some(core) = supervised_owner.upgrade() {
                        let _ = core.lock().unwrap_or_else(|e| e.into_inner()).registry.stop(&supervised_listener);
                    }
                }
                joined
            });
            *resource.startup.lock().unwrap_or_else(|e| e.into_inner()) = Some(supervisor.map(|result| result.unwrap_or(false)).boxed().shared());
            resource
        } else {
            core.resources.get(&acquisition.listener).expect("registry resource ownership").clone()
        };
        Ok(Reservation { acquisition, resource, core: Arc::downgrade(&self.core), connection })
    }

    pub async fn release(&self, connection: Connection, epoch: &str, lease: &str, deadline: Instant) -> Result<()> {
        let listener = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.registry.release(connection, epoch, lease).map_err(|error| anyhow!("release rejected: {error:?}"))?
        };
        if let Some(listener) = listener { self.drain(&listener, deadline).await?; }
        Ok(())
    }
    pub async fn revoke(&self, policy: [u8; 32], generation: u64, deadline: Instant) -> Result<()> {
        let listeners = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let listeners = core.registry.revoke_domain(policy, generation).map_err(|_| anyhow!("revoke rejected"))?;
            for listener in &listeners {
                if let Some(resource) = core.resources.get(listener) { resource.cancel(); }
            }
            listeners
        };
        // Start every revoked resource's drain even if one cannot confirm
        // cleanup. One stuck route must not keep the others listening.
        let results = futures::future::join_all(listeners.iter().map(|listener| self.drain(listener, deadline))).await;
        ensure!(results.iter().all(Result::is_ok), "cleanup_unconfirmed: revoked resources remain stopping");
        Ok(())
    }
    pub async fn disconnect(&self, connection: Connection, deadline: Instant) -> Result<()> {
        let listeners = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            // Retire physically completed AND actually joined operations even
            // if their original caller never retries. The display-history FIFO
            // is not an authority for pending operation completion.
            core.retire_disconnections();
            match core.disconnections.get(&connection).cloned() {
                Some(Disconnection::Complete) => return Ok(()),
                Some(Disconnection::Pending(listeners)) => listeners,
                None => {
                    ensure!(core.registry.has_connection(connection), "connection unavailable");
                    if core.disconnections.len() >= 256 {
                        let expired = core.disconnections.iter().find_map(|(id, state)|
                            matches!(state, Disconnection::Complete).then_some(*id));
                        if let Some(expired) = expired { core.disconnections.remove(&expired); }
                        else { return Err(anyhow!("disconnect completion capacity reached")); }
                    }
                    let listeners = core.registry.disconnect(connection);
                    let listeners = listeners.into_iter().map(|listener| {
                        let resource = core.resources.get(&listener).cloned()
                            .ok_or_else(|| anyhow!("cleanup_unconfirmed: resource ownership missing"))?;
                        resource.cancel();
                        Ok((listener, resource))
                    }).collect::<Result<Vec<_>>>()?;
                    // Retain the exact resources/proofs after registry removes leases.
                    // A repeated disconnect must join them, not succeed on an
                    // empty second registry.disconnect result.
                    core.disconnections.insert(connection, Disconnection::Pending(listeners.clone()));
                    listeners
                }
            }
        };
        let results = futures::future::join_all(listeners.iter().map(|(listener, resource)|
            self.drain_resource(listener, resource.clone(), deadline))).await;
        ensure!(results.iter().all(Result::is_ok), "cleanup_unconfirmed: disconnected resources remain stopping");
        if let Some(state) = self.core.lock().unwrap_or_else(|e| e.into_inner()).disconnections.get_mut(&connection) {
            *state = Disconnection::Complete;
        }
        Ok(())
    }

    pub async fn drain(&self, listener: &str, deadline: Instant) -> Result<()> {
        let (resource, completed) = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(resource) = core.resources.get(listener).cloned() {
                ensure!(core.registry.is_stopping(listener), "resource is not stopping");
                (Some(resource), None)
            } else {
                let completion = core.completed.iter().find(|(id, _)| id == listener).map(|(_, task)| task.clone())
                    .ok_or_else(|| anyhow!("resource completion unavailable"))?;
                (None, Some(completion))
            }
        };
        if let Some(completed) = completed {
            ensure!(tokio::time::timeout_at(deadline, completed).await
                .map_err(|_| anyhow!("cleanup_unconfirmed: completion join deadline expired"))?,
                "cleanup_unconfirmed: completion task failed");
            return Ok(());
        }
        self.drain_resource(listener, resource.expect("live resource or retained completion"), deadline).await
    }

    async fn drain_resource(&self, listener: &str, resource: Arc<Resource>, deadline: Instant) -> Result<()> {
        resource.cancel();
        tokio::time::timeout_at(deadline, async {
            let _drain = resource.drain_lock.lock().await;
            let startup = resource.startup.lock().unwrap_or_else(|e| e.into_inner()).clone()
                .ok_or_else(|| anyhow!("cleanup_unconfirmed: startup registration missing"))?;
            ensure!(startup.clone().await, "cleanup_unconfirmed: resource task failed");
            if resource.cleanup_confirmed.load(Ordering::Acquire) { return Ok(()); }
            let mut runtime = resource.runtime.lock().await;
            // A normally joined worker can still have returned a failed
            // physical drain. Neither true join nor missing cache is success.
            let owned = runtime.as_mut().ok_or_else(|| anyhow!("cleanup_unconfirmed: physical completion missing"))?;
            owned.drain_until(deadline).await?;
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            // Never free a bind after timeout or merely after sending stop.
            core.confirmed_drained(listener, startup).map_err(|_| anyhow!("cleanup_unconfirmed: stopping ownership changed"))?;
            runtime.take();
            Ok(())
        }).await.map_err(|_| anyhow!("cleanup_unconfirmed: coordinator drain deadline expired"))?
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use std::time::Duration;

    // Controlled registry resources have no sockets. Their real Tokio join
    // tasks isolate completion-proof/cache invariants; the three-hop tests in
    // runtime.rs remain the evidence for actual network cleanup.
    fn install_controlled(coordinator: &Coordinator, serial: u16, joined: StartupJoin)
        -> (Connection, String, Arc<Resource>) {
        let connection = coordinator.connect_saved([7; 32], 3).unwrap();
        let mut core = coordinator.core.lock().unwrap();
        let epoch = core.registry.epoch().to_owned();
        let acquisition = core.registry.acquire(connection, &epoch, RouteKey {
            session_id: format!("controlled-{serial}"), forward_index: 0,
            revision: [4; 32], permission_generation: 3,
        }, SocketAddr::from(([127, 0, 0, 1], 20000 + serial)), true).unwrap();
        let resource = Arc::new(Resource {
            control: Mutex::new(Control::default()), runtime: tokio::sync::Mutex::new(None),
            drain_lock: tokio::sync::Mutex::new(()), startup: Mutex::new(Some(joined)),
            state: watch::channel(Startup::Pending).0, cleanup_confirmed: AtomicBool::new(false),
        });
        core.resources.insert(acquisition.listener.clone(), resource.clone());
        (connection, acquisition.listener, resource)
    }
    fn completed_task() -> StartupJoin {
        tokio::spawn(async { true }).map(|result| result.unwrap_or(false)).boxed().shared()
    }
    async fn advance_history(coordinator: &Coordinator) {
        for serial in 1..=129 {
            let joined = completed_task(); assert!(joined.clone().await);
            let (connection, listener, _) = install_controlled(coordinator, serial, joined.clone());
            let mut core = coordinator.core.lock().unwrap();
            assert_eq!(core.registry.disconnect(connection), vec![listener.clone()]);
            core.confirmed_drained(&listener, joined).unwrap();
        }
        assert_eq!(coordinator.core.lock().unwrap().completed.len(), 128);
    }

    #[tokio::test]
    async fn timed_out_disconnect_retains_proof_after_completion_fifo_eviction() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let coordinator = Coordinator::new(); coordinator.authorize_saved([7; 32], 3).unwrap();
            let (release, blocked) = tokio::sync::oneshot::channel::<()>();
            let joined = tokio::spawn(async move { blocked.await.is_ok() })
                .map(|result| result.unwrap_or(false)).boxed().shared();
            let (connection, listener, resource) = install_controlled(&coordinator, 0, joined.clone());
            assert!(coordinator.disconnect(connection, Instant::now() + Duration::from_millis(10)).await.is_err());
            release.send(()).unwrap(); assert!(joined.clone().await);
            // No network resources exist in this fixture; joining the owned
            // task completes its physical obligations before publishing proof.
            coordinator.core.lock().unwrap().confirmed_drained(&listener, joined).unwrap();
            advance_history(&coordinator).await;
            assert!(!coordinator.core.lock().unwrap().completed.iter().any(|(id, _)| id == &listener));
            assert!(resource.cleanup_confirmed.load(Ordering::Acquire));
            coordinator.disconnect(connection, Instant::now() + Duration::from_secs(1)).await.unwrap();
            assert!(matches!(coordinator.core.lock().unwrap().disconnections.get(&connection), Some(Disconnection::Complete)));
        }).await.expect("completion FIFO fixture exceeded deadline");
    }

    #[tokio::test]
    async fn successful_worker_join_without_physical_cleanup_never_acknowledges_disconnect() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let coordinator = Coordinator::new(); coordinator.authorize_saved([7; 32], 3).unwrap();
            let joined = completed_task(); assert!(joined.clone().await);
            let (connection, listener, resource) = install_controlled(&coordinator, 0, joined);
            for _ in 0..2 {
                assert!(coordinator.disconnect(connection, Instant::now() + Duration::from_secs(1)).await
                    .unwrap_err().to_string().contains("cleanup_unconfirmed"));
            }
            advance_history(&coordinator).await;
            assert!(coordinator.disconnect(connection, Instant::now() + Duration::from_secs(1)).await.is_err());
            assert!(!resource.cleanup_confirmed.load(Ordering::Acquire));
            let core = coordinator.core.lock().unwrap();
            assert!(core.registry.is_stopping(&listener));
            assert!(matches!(core.disconnections.get(&connection), Some(Disconnection::Pending(_))));
        }).await.expect("unconfirmed cleanup fixture exceeded deadline");
    }

    #[tokio::test]
    async fn completed_pending_operations_retire_before_disconnect_capacity_check() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let coordinator = Coordinator::new(); coordinator.authorize_saved([7; 32], 3).unwrap();
            let joined = completed_task(); assert!(joined.clone().await);
            let (connection, listener, resource) = install_controlled(&coordinator, 0, joined.clone());
            {
                let mut core = coordinator.core.lock().unwrap();
                core.registry.disconnect(connection);
                core.confirmed_drained(&listener, joined).unwrap();
                // Multiple timed-out operations can retain the same completed
                // proof. None is eligible solely because its worker joined;
                // confirmed physical cleanup is also required.
                for id in 1000..1256 {
                    core.disconnections.insert(id, Disconnection::Pending(vec![(listener.clone(), resource.clone())]));
                }
            }
            let fresh = coordinator.connect_saved([7; 32], 3).unwrap();
            coordinator.disconnect(fresh, Instant::now() + Duration::from_secs(1)).await.unwrap();
            let core = coordinator.core.lock().unwrap();
            assert_eq!(core.disconnections.len(), 256);
            assert!(core.disconnections.values().all(|state| matches!(state, Disconnection::Complete)));
        }).await.expect("disconnect capacity retirement fixture exceeded deadline");
    }
}
