//! Serialized ownership transitions, separate from network I/O. The broker
//! event loop owns this registry. Resource cleanup happens outside the lock,
//! then reports drain completion; stopping entries continue reserving binds.
use super::protocol::Error;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use uuid::Uuid;

pub(super) type Connection = u64;

/// Constructed by an internal adapter bootstrap after OS peer verification,
/// never deserialized from a request. OS identity alone does not attest which
/// program a same-user process is running.
#[derive(Clone, Copy)]
pub(super) enum AdapterMode { LocalSaved, HttpIngress }

pub(super) struct TrustedAdapterRegistration {
    pub(super) mode: AdapterMode,
    // A digest of the server-resolved saved authorization policy, NOT a
    // client-provided owner ID. Profile identity belongs to the broker itself.
    pub(super) saved_policy: [u8; 32],
    pub(super) authorization_generation: u64,
}

struct AuthorizationCycle {
    generation: u64,
    revoked: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RouteKey {
    pub session_id: String,
    pub forward_index: usize,
    pub revision: [u8; 32],
    pub permission_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State { Pending, Running, ConfigurationStale, Stopping }

struct Listener {
    policy: [u8; 32],
    route: RouteKey,
    bind: SocketAddr,
    state: State,
    leases: HashSet<String>,
}

struct Lease {
    connection: Connection,
    listener: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Acquisition {
    pub listener: String,
    pub lease: String,
    pub start: bool,
}

pub(super) struct Registry {
    epoch: String,
    connections: HashMap<Connection, TrustedAdapterRegistration>,
    authorizations: HashMap<[u8; 32], AuthorizationCycle>,
    listeners: HashMap<String, Listener>,
    leases: HashMap<String, Lease>,
    next_connection: u64,
    exiting: bool,
}

impl Registry {
    pub fn new() -> Self {
        Self { epoch: Uuid::new_v4().to_string(), connections: HashMap::new(),
            authorizations: HashMap::new(), listeners: HashMap::new(), leases: HashMap::new(), next_connection: 1, exiting: false }
    }

    /// Only trusted policy evaluation can open a new authorization cycle.
    /// A revoked generation remains a tombstone after every listener drains.
    /// Merely reconnecting, acquiring or choosing a higher route generation
    /// cannot authorize anything. Old connections retain their original cycle.
    pub(super) fn authorize_domain(&mut self, policy: [u8; 32], generation: u64) -> Result<(), Error> {
        if let Some(previous) = self.authorizations.get(&policy) {
            if !previous.revoked || generation <= previous.generation { return Err(Error::Revoked); }
        } else if self.authorizations.len() >= 128 { return Err(Error::Busy); }
        self.authorizations.insert(policy, AuthorizationCycle { generation, revoked: false });
        Ok(())
    }

    fn cycle_live(&self, policy: &[u8; 32], generation: u64) -> bool {
        self.authorizations.get(policy).is_some_and(|cycle|
            !cycle.revoked && cycle.generation == generation)
    }

    /// Private until a verified OS IPC bootstrap is implemented. No production
    /// caller can register unverified wire input as local authority.
    pub(super) fn connect(&mut self, registration: TrustedAdapterRegistration) -> Result<Connection, Error> {
        if self.exiting { return Err(Error::Stopping); }
        if self.connections.len() >= 128 { return Err(Error::Busy); }
        if matches!(registration.mode, AdapterMode::LocalSaved)
            && !self.cycle_live(&registration.saved_policy, registration.authorization_generation) {
            return Err(Error::Revoked);
        }
        let connection = self.next_connection;
        self.next_connection = self.next_connection.checked_add(1).ok_or(Error::Busy)?;
        self.connections.insert(connection, registration);
        Ok(connection)
    }

    fn local_policy(&self, connection: Connection) -> Result<[u8; 32], Error> {
        let registration = self.connections.get(&connection).ok_or(Error::Unauthorized)?;
        match registration.mode {
            AdapterMode::LocalSaved => Ok(registration.saved_policy),
            // HTTP adapter is intentionally not part of this first slice.
            // Unsupported must not fall through to LocalSaved.
            AdapterMode::HttpIngress => Err(Error::Unsupported),
        }
    }

    pub fn acquire(&mut self, connection: Connection, epoch: &str, route: RouteKey,
        bind: SocketAddr, ensure: bool) -> Result<Acquisition, Error> {
        if epoch != self.epoch { return Err(Error::StaleEpoch); }
        let policy = self.local_policy(connection)?;
        let generation = self.connections.get(&connection).ok_or(Error::Unauthorized)?.authorization_generation;
        if !self.cycle_live(&policy, generation) || route.permission_generation != generation {
            return Err(Error::Revoked);
        }
        if self.leases.len() >= 1024 { return Err(Error::Busy); }
        // A changed bind must not bypass the existing session/rule revision
        // guard by looking only at the new socket address.
        let existing = self.listeners.iter().find(|(_, value)|
            value.policy == policy && value.route.session_id == route.session_id
                && value.route.forward_index == route.forward_index)
            .or_else(|| self.listeners.iter().find(|(_, value)| value.bind == bind))
            .map(|(id, value)| (id.clone(), value));
        let (id, start) = if let Some((id, value)) = existing {
            if value.policy != policy { return Err(Error::PortInUse); }
            if value.state == State::Stopping { return Err(Error::Stopping); }
            if value.route != route || value.bind != bind || value.state == State::ConfigurationStale {
                return Err(Error::ConfigChanged);
            }
            if !ensure { return Err(Error::PortInUse); }
            (id, false)
        } else {
            if self.listeners.len() >= 32 { return Err(Error::Busy); }
            let id = Uuid::new_v4().to_string();
            self.listeners.insert(id.clone(), Listener { policy, route, bind,
                state: State::Pending, leases: HashSet::new() });
            (id, true)
        };
        let lease = Uuid::new_v4().to_string();
        self.listeners.get_mut(&id).unwrap().leases.insert(lease.clone());
        self.leases.insert(lease.clone(), Lease { connection, listener: id.clone() });
        Ok(Acquisition { listener: id, lease, start })
    }

    /// Called only after startup and a fresh permission/trust generation check.
    /// A revoked or cancelled startup can never publish a late ready.
    pub fn ready(&mut self, listener: &str, generation: u64) -> Result<(), Error> {
        let value = self.listeners.get(listener).ok_or(Error::NotFound)?;
        if !self.cycle_live(&value.policy, generation) { return Err(Error::Revoked); }
        let value = self.listeners.get_mut(listener).unwrap();
        if value.state != State::Pending || value.leases.is_empty()
            || value.route.permission_generation != generation {
            return Err(Error::Revoked);
        }
        value.state = State::Running;
        Ok(())
    }

    /// Returns the listener needing asynchronous drain, if this was its last
    /// reference. The caller must await drain before acknowledging release.
    pub fn release(&mut self, connection: Connection, epoch: &str, lease: &str)
        -> Result<Option<String>, Error> {
        if epoch != self.epoch { return Err(Error::StaleEpoch); }
        let value = self.leases.get(lease).ok_or(Error::NotFound)?;
        if value.connection != connection { return Err(Error::Unauthorized); }
        let value = self.leases.remove(lease).unwrap();
        let listener = self.listeners.get_mut(&value.listener).ok_or(Error::NotFound)?;
        listener.leases.remove(lease);
        if listener.leases.is_empty() {
            listener.state = State::Stopping;
            Ok(Some(value.listener))
        } else { Ok(None) }
    }

    pub fn disconnect(&mut self, connection: Connection) -> Vec<String> {
        let leases: Vec<_> = self.leases.iter().filter(|(_, value)| value.connection == connection)
            .map(|(id, _)| id.clone()).collect();
        let epoch = self.epoch.clone();
        let drains = leases.iter().filter_map(|lease| self.release(connection, &epoch, lease).ok().flatten()).collect();
        self.connections.remove(&connection);
        drains
    }

    /// Ordinary route changes preserve old leases' immutable snapshots, but
    /// disallow new leases. Security revocation uses revoke instead.
    pub fn configuration_stale(&mut self, listener: &str) -> Result<(), Error> {
        let value = self.listeners.get_mut(listener).ok_or(Error::NotFound)?;
        if value.state == State::Running { value.state = State::ConfigurationStale; }
        else if value.state == State::Pending { value.state = State::Stopping; }
        Ok(())
    }

    /// Trusted policy evaluation revokes a cycle independently of resource
    /// existence. In particular, the last listener draining does not remove
    /// the ability to revoke an otherwise idle authorized connection.
    pub(super) fn revoke_domain(&mut self, policy: [u8; 32], generation: u64) -> Result<Vec<String>, Error> {
        let cycle = self.authorizations.get_mut(&policy).ok_or(Error::NotFound)?;
        if cycle.generation != generation { return Err(Error::Revoked); }
        cycle.revoked = true;
        let mut drains = Vec::new();
        for (id, value) in &mut self.listeners {
            if value.policy == policy && value.route.permission_generation == generation {
                value.state = State::Stopping;
                drains.push(id.clone());
            }
        }
        Ok(drains)
    }

    /// A resource-based convenience for internal callers, not the sole policy
    /// revocation entry point. Return every affected resource for drain; no
    /// success ACK until all resources are actually closed.
    pub fn revoke(&mut self, listener: &str) -> Result<Vec<String>, Error> {
        let value = self.listeners.get(listener).ok_or(Error::NotFound)?;
        self.revoke_domain(value.policy, value.route.permission_generation)
    }

    /// This is not a cancellation request: the runtime must have already
    /// joined pending work, accepted clients and every SSH transport.
    pub fn drained(&mut self, listener: &str) -> Result<(), Error> {
        let value = self.listeners.get(listener).ok_or(Error::NotFound)?;
        if value.state != State::Stopping { return Err(Error::Busy); }
        let value = self.listeners.remove(listener).unwrap();
        for lease in value.leases { self.leases.remove(&lease); }
        Ok(())
    }

    /// Snapshot recovers this connection's leases after a lost acquire reply.
    /// Local view deliberately contains no other connection's lease tokens.
    pub fn snapshot(&self, connection: Connection) -> Result<Vec<(String, String)>, Error> {
        self.local_policy(connection)?;
        Ok(self.leases.iter().filter(|(_, value)| value.connection == connection)
            .map(|(id, value)| (id.clone(), value.listener.clone())).collect())
    }

    pub fn local_view(&self, connection: Connection) -> Result<Vec<(String, State, usize)>, Error> {
        self.local_policy(connection)?;
        Ok(self.listeners.iter().map(|(id, value)| (id.clone(), value.state, value.leases.len())).collect())
    }

    pub(super) fn has_connection(&self, connection: Connection) -> bool { self.connections.contains_key(&connection) }
    pub(super) fn epoch(&self) -> &str { &self.epoch }
    pub(super) fn is_stopping(&self, listener: &str) -> bool {
        self.listeners.get(listener).is_some_and(|value| value.state == State::Stopping)
    }
    pub(super) fn live(&self, listener: &str) -> bool {
        self.listeners.get(listener).is_some_and(|value|
            matches!(value.state, State::Pending | State::Running | State::ConfigurationStale)
                && self.cycle_live(&value.policy, value.route.permission_generation)
                && !value.leases.is_empty())
    }
    pub(super) fn stop(&mut self, listener: &str) -> Result<(), Error> {
        let value = self.listeners.get_mut(listener).ok_or(Error::NotFound)?;
        value.state = State::Stopping;
        Ok(())
    }

    /// Same serialized transition as connect. Once exiting is chosen, later
    /// connections are rejected instead of racing a new acquisition with exit.
    pub fn begin_idle_exit(&mut self) -> bool {
        if !self.connections.is_empty() || !self.listeners.is_empty() { return false; }
        self.exiting = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn connect(registry: &mut Registry) -> Connection {
        if !registry.authorizations.contains_key(&[1; 32]) {
            registry.authorize_domain([1; 32], 3).unwrap();
        }
        registry.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved, saved_policy: [1; 32], authorization_generation: 3 }).unwrap()
    }
    fn route() -> RouteKey {
        RouteKey { session_id: "fixture".into(), forward_index: 0, revision: [2; 32], permission_generation: 3 }
    }
    fn acquire(registry: &mut Registry, connection: Connection) -> Acquisition {
        registry.acquire(connection, &registry.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true).unwrap()
    }
    #[test]
    fn independent_connections_share_listener_not_release_authority() {
        let mut r = Registry::new(); let a = connect(&mut r); let b = connect(&mut r);
        let first = acquire(&mut r, a); let second = acquire(&mut r, b);
        assert!(first.start); assert!(!second.start);
        assert_eq!(first.listener, second.listener); assert_ne!(first.lease, second.lease);
        assert_eq!(r.local_view(a).unwrap(), r.local_view(b).unwrap());
        assert_eq!(r.release(b, &r.epoch.clone(), &first.lease), Err(Error::Unauthorized));
        assert!(r.disconnect(a).is_empty());
        r.ready(&second.listener, 3).unwrap();
        assert_eq!(r.snapshot(b).unwrap(), vec![(second.lease.clone(), second.listener.clone())]);
        assert_eq!(r.disconnect(b), vec![second.listener.clone()]);
        assert!(!r.begin_idle_exit());
        r.drained(&second.listener).unwrap(); assert!(r.begin_idle_exit());
    }
    #[test]
    fn last_release_reserves_bind_until_drain_and_blocks_late_ready() {
        let mut r = Registry::new(); let a = connect(&mut r); let old = acquire(&mut r, a);
        r.release(a, &r.epoch.clone(), &old.lease).unwrap();
        assert_eq!(r.ready(&old.listener, 3), Err(Error::Revoked));
        assert_eq!(r.acquire(a, &r.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true), Err(Error::Stopping));
        r.drained(&old.listener).unwrap(); let new = acquire(&mut r, a);
        assert_ne!(old.listener, new.listener);
    }
    #[test]
    fn http_registration_never_falls_back_to_local() {
        let mut r = Registry::new(); let c = r.connect(TrustedAdapterRegistration {
            mode: AdapterMode::HttpIngress, saved_policy: [1; 32], authorization_generation: 3 }).unwrap();
        assert_eq!(r.local_view(c), Err(Error::Unsupported));
        assert_eq!(r.acquire(c, &r.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true), Err(Error::Unsupported));
    }
    #[test]
    fn stale_route_and_security_revocation_cannot_gain_new_leases() {
        let mut r = Registry::new(); let c = connect(&mut r); let a = acquire(&mut r, c);
        r.ready(&a.listener, 3).unwrap(); r.configuration_stale(&a.listener).unwrap();
        assert_eq!(r.acquire(c, &r.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true), Err(Error::ConfigChanged));
        assert_eq!(r.snapshot(c).unwrap().len(), 1);
        r.revoke(&a.listener).unwrap(); assert_eq!(r.ready(&a.listener, 3), Err(Error::Revoked));
        r.drained(&a.listener).unwrap(); assert!(r.snapshot(c).unwrap().is_empty());
    }
    #[test]
    fn different_policy_cannot_share_bind_and_epoch_cannot_be_replayed() {
        let mut r = Registry::new(); let a = connect(&mut r); let lease = acquire(&mut r, a);
        r.authorize_domain([9; 32], 3).unwrap();
        let b = r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved, saved_policy: [9; 32], authorization_generation: 3 }).unwrap();
        assert_eq!(r.acquire(b, &r.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true), Err(Error::PortInUse));
        assert_eq!(r.release(a, "old-epoch", &lease.lease), Err(Error::StaleEpoch));
    }
    #[test]
    fn accepted_connection_prevents_idle_exit_and_exit_prevents_accept() {
        let mut r = Registry::new(); let c = connect(&mut r); assert!(!r.begin_idle_exit());
        r.disconnect(c); assert!(r.begin_idle_exit());
        assert_eq!(r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved, saved_policy: [1; 32], authorization_generation: 3 }), Err(Error::Stopping));
    }
    #[tokio::test]
    async fn cancellation_barrier_does_not_cancel_another_connections_ready() {
        use std::sync::Arc;
        use tokio::sync::{Barrier, Mutex};
        let mut initial = Registry::new();
        let a = connect(&mut initial); let b = connect(&mut initial);
        let first = acquire(&mut initial, a); let second = acquire(&mut initial, b);
        let shared = Arc::new(Mutex::new(initial));
        let barrier = Arc::new(Barrier::new(2));
        let left = shared.clone(); let cancelled = barrier.clone();
        let task = tokio::spawn(async move {
            assert!(left.lock().await.disconnect(a).is_empty());
            cancelled.wait().await;
        });
        barrier.wait().await;
        let mut registry = shared.lock().await;
        registry.ready(&second.listener, 3).unwrap();
        assert_eq!(first.listener, second.listener);
        assert_eq!(registry.snapshot(b).unwrap().len(), 1);
        drop(registry); task.await.unwrap();
    }

    #[tokio::test]
    async fn revoke_between_authentication_and_ready_prevents_publication() {
        use std::sync::Arc;
        use tokio::sync::{Barrier, Mutex};
        let mut initial = Registry::new(); let c = connect(&mut initial);
        let acquired = acquire(&mut initial, c);
        let listener = acquired.listener.clone();
        let shared = Arc::new(Mutex::new(initial));
        let authenticated = Arc::new(Barrier::new(2));
        let revoked = Arc::new(Barrier::new(2));
        let task_registry = shared.clone();
        let task_authenticated = authenticated.clone(); let task_revoked = revoked.clone();
        let task = tokio::spawn(async move {
            task_authenticated.wait().await;
            task_revoked.wait().await;
            task_registry.lock().await.ready(&listener, 3)
        });
        authenticated.wait().await;
        shared.lock().await.revoke(&acquired.listener).unwrap();
        revoked.wait().await;
        assert_eq!(task.await.unwrap(), Err(Error::Revoked));
    }

    #[test]
    fn changing_saved_bind_cannot_bypass_existing_rule_revision() {
        let mut r = Registry::new(); let c = connect(&mut r); let old = acquire(&mut r, c);
        r.ready(&old.listener, 3).unwrap();
        let mut changed = route(); changed.revision = [7; 32];
        assert_eq!(r.acquire(c, &r.epoch.clone(), changed, "127.0.0.1:1081".parse().unwrap(), true), Err(Error::ConfigChanged));
        assert_eq!(r.snapshot(c).unwrap().len(), 1);
    }

    #[test]
    fn revoked_cycle_cannot_resurrect_after_drain_or_by_reconnecting() {
        let mut r = Registry::new(); let c = connect(&mut r); let old = acquire(&mut r, c);
        r.ready(&old.listener, 3).unwrap();
        assert_eq!(r.revoke(&old.listener).unwrap(), vec![old.listener.clone()]);
        r.drained(&old.listener).unwrap();
        let bind = "127.0.0.1:1080".parse().unwrap();
        assert_eq!(r.acquire(c, &r.epoch.clone(), route(), bind, true), Err(Error::Revoked));
        assert_eq!(r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved,
            saved_policy: [1; 32], authorization_generation: 3 }), Err(Error::Revoked));
        assert_eq!(r.authorize_domain([1; 32], 3), Err(Error::Revoked));
        let mut new_route = route(); new_route.permission_generation = 4;
        // A caller-supplied higher route generation is not reauthorization.
        assert_eq!(r.acquire(c, &r.epoch.clone(), new_route.clone(), bind, true), Err(Error::Revoked));
        r.authorize_domain([1; 32], 4).unwrap();
        assert_eq!(r.acquire(c, &r.epoch.clone(), new_route.clone(), bind, true), Err(Error::Revoked));
        let fresh = r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved,
            saved_policy: [1; 32], authorization_generation: 4 }).unwrap();
        let next = r.acquire(fresh, &r.epoch.clone(), new_route, bind, true).unwrap();
        assert_eq!(r.ready(&next.listener, 3), Err(Error::Revoked));
        r.ready(&next.listener, 4).unwrap();
        assert_ne!(old.listener, next.listener);
    }

    #[test]
    fn domain_revocation_stops_all_its_rules_but_not_an_unrelated_domain() {
        let mut r = Registry::new(); let c = connect(&mut r); let a = acquire(&mut r, c);
        let mut other_rule = route(); other_rule.forward_index = 1;
        let b = r.acquire(c, &r.epoch.clone(), other_rule, "127.0.0.1:1081".parse().unwrap(), true).unwrap();
        r.authorize_domain([9; 32], 3).unwrap();
        let other = r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved,
            saved_policy: [9; 32], authorization_generation: 3 }).unwrap();
        let independent = r.acquire(other, &r.epoch.clone(), route(), "127.0.0.1:1082".parse().unwrap(), true).unwrap();
        r.ready(&a.listener, 3).unwrap(); r.ready(&independent.listener, 3).unwrap();
        let drains: HashSet<_> = r.revoke(&a.listener).unwrap().into_iter().collect();
        assert_eq!(drains, HashSet::from([a.listener.clone(), b.listener.clone()]));
        assert_eq!(r.listeners[&a.listener].state, State::Stopping);
        assert_eq!(r.ready(&b.listener, 3), Err(Error::Revoked));
        assert_eq!(r.listeners[&independent.listener].state, State::Running);
        r.drained(&a.listener).unwrap(); r.drained(&b.listener).unwrap();
        assert_eq!(r.snapshot(other).unwrap().len(), 1);
        assert!(r.snapshot(c).unwrap().is_empty());
    }

    #[test]
    fn domain_can_be_revoked_without_ever_having_a_listener() {
        let mut r = Registry::new(); let c = connect(&mut r);
        assert!(r.revoke_domain([1; 32], 3).unwrap().is_empty());
        assert_eq!(r.acquire(c, &r.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true), Err(Error::Revoked));
        assert_eq!(r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved,
            saved_policy: [1; 32], authorization_generation: 3 }), Err(Error::Revoked));
    }

    #[test]
    fn domain_revocation_after_ordinary_drain_still_invalidates_idle_connections() {
        let mut r = Registry::new(); let c = connect(&mut r); let a = acquire(&mut r, c);
        r.release(c, &r.epoch.clone(), &a.lease).unwrap();
        r.drained(&a.listener).unwrap();
        assert!(r.revoke_domain([1; 32], 3).unwrap().is_empty());
        assert_eq!(r.acquire(c, &r.epoch.clone(), route(), "127.0.0.1:1080".parse().unwrap(), true), Err(Error::Revoked));
        // Delayed notifications from an old cycle cannot revoke a newer one.
        r.authorize_domain([1; 32], 4).unwrap();
        assert_eq!(r.revoke_domain([1; 32], 3), Err(Error::Revoked));
        assert!(r.cycle_live(&[1; 32], 4));
    }

    #[test]
    fn trusted_pending_domain_revocation_blocks_ready_and_preserves_other_domain() {
        let mut r = Registry::new(); let c = connect(&mut r); let pending = acquire(&mut r, c);
        r.authorize_domain([9; 32], 3).unwrap();
        assert_eq!(r.revoke_domain([1; 32], 3).unwrap(), vec![pending.listener.clone()]);
        assert_eq!(r.ready(&pending.listener, 3), Err(Error::Revoked));
        assert!(r.cycle_live(&[9; 32], 3));
        r.drained(&pending.listener).unwrap();
        assert!(r.revoke_domain([1; 32], 3).unwrap().is_empty());
    }

    #[test]
    fn bounded_authorization_tombstones_fail_closed_without_eviction() {
        let mut r = Registry::new();
        for byte in 0..128u8 {
            r.authorize_domain([byte; 32], 3).unwrap();
            assert!(r.revoke_domain([byte; 32], 3).unwrap().is_empty());
        }
        assert_eq!(r.authorize_domain([128; 32], 3), Err(Error::Busy));
        assert_eq!(r.authorize_domain([0; 32], 3), Err(Error::Revoked));
        assert_eq!(r.connect(TrustedAdapterRegistration { mode: AdapterMode::LocalSaved,
            saved_policy: [0; 32], authorization_generation: 3 }), Err(Error::Revoked));
        r.authorize_domain([0; 32], 4).unwrap();
        assert!(r.cycle_live(&[0; 32], 4));
        assert_eq!(r.authorizations.len(), 128);
    }

}
