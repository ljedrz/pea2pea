//! Node-level operational signals the library observes internally.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[cfg(doc)]
use crate::{
    Config, DisconnectOrigin, Node, Stats,
    protocols::{Handshake, Reading, Writing},
};
#[cfg(doc)]
use std::io::ErrorKind;

/// A collection of node-level operational/health signals.
#[derive(Default)]
pub struct Heuristics {
    /// Outbound dials rejected because the shared connection-setup budget was exhausted.
    connect_budget_rejections: AtomicU64,
    /// Inbound connections turned away by admission control before any protocol ran.
    inbound_connections_rejected: AtomicU64,
    /// Failures of the OS `accept` call that indicate local resource pressure.
    accept_errors: AtomicU64,
    /// Connections dropped because the handshake exceeded its time budget.
    handshake_timeouts: AtomicU64,
    /// Connections dropped because no inbound message arrived within the idle timeout.
    idle_timeouts: AtomicU64,
    /// Connections dropped because an outbound write exceeded its flush time budget.
    write_timeouts: AtomicU64,
}

impl Heuristics {
    /// Registers a single outbound-connect rejection caused by connection-setup budget exhaustion.
    pub(crate) fn register_connect_budget_rejection(&self) {
        self.connect_budget_rejections.fetch_add(1, Relaxed);
    }

    /// Returns the number of [`Node::connect`] attempts that were rejected because the shared
    /// connection-setup budget ([`Config::max_connecting`]) was exhausted at the time of the call.
    ///
    /// note: This counts *every* budget rejection. Although inbound accepts and outbound connects
    /// draw from the same budget, only outbound dials are ever *rejected* by it - an inbound accept
    /// that finds the budget full is instead *backpressured* (it waits for a slot, surplus peers
    /// queuing in the OS accept queue), so there is no inbound rejection event to count. A rising
    /// *rate* here while the node's own dial rate is modest is therefore a strong indicator of
    /// inbound-side saturation (e.g. a connection flood) crowding out the shared budget. Sample it
    /// periodically and watch the slope rather than the absolute value.
    pub fn connect_budget_rejections(&self) -> u64 {
        self.connect_budget_rejections.load(Relaxed)
    }

    /// Registers a single inbound connection rejected by admission control.
    pub(crate) fn register_inbound_rejection(&self) {
        self.inbound_connections_rejected.fetch_add(1, Relaxed);
    }

    /// Returns the number of inbound connections that were turned away by the node's admission
    /// control - the per-IP cap ([`Config::max_connections_per_ip`]), the global cap
    /// ([`Config::max_connections`]), or a duplicate of an existing/pending connection - *before*
    /// any [`Handshake`] or other protocol ran for them. The pending-connection cap
    /// ([`Config::max_connecting`]) never rejects inbound connections - it backpressures them
    /// (see [`Heuristics::connect_budget_rejections`]).
    ///
    /// note: Such connections never reach a user hook (they are refused inside the accept loop), so
    /// this is otherwise entirely invisible to the application. A rising *rate* is the most direct
    /// signal of inbound-side pressure or a connection flood - more immediate than
    /// [`Heuristics::connect_budget_rejections`], which only reflects the flood's side effect on the
    /// node's own outbound dialing. It excludes connections rejected merely because the node is
    /// shutting down.
    pub fn inbound_connections_rejected(&self) -> u64 {
        self.inbound_connections_rejected.load(Relaxed)
    }

    /// Registers a single resource-pressure failure of the OS `accept` call.
    pub(crate) fn register_accept_error(&self) {
        self.accept_errors.fetch_add(1, Relaxed);
    }

    /// Returns the number of times the OS `accept` call failed in a way that indicates local resource
    /// pressure (typically file-descriptor or memory exhaustion, e.g. `EMFILE`/`ENFILE`/`ENOBUFS`),
    /// each of which also triggers a short backoff in the accept loop.
    ///
    /// note: Benign, transient peer-side aborts ([`ErrorKind::ConnectionAborted`] /
    /// [`ErrorKind::ConnectionReset`] before `accept` completes) are **not** counted, so any nonzero
    /// value here reflects a genuine local resource problem the operator should act on (e.g. raise
    /// the process's file-descriptor limit) rather than normal churn.
    pub fn accept_errors(&self) -> u64 {
        self.accept_errors.load(Relaxed)
    }

    /// Registers a single connection dropped due to a handshake timeout.
    pub(crate) fn register_handshake_timeout(&self) {
        self.handshake_timeouts.fetch_add(1, Relaxed);
    }

    /// Returns the number of connections dropped because their [`Handshake`] did not complete within
    /// [`Handshake::TIMEOUT_MS`](Handshake::TIMEOUT_MS).
    ///
    /// note: Only *timeouts* are counted here - a handshake that fails because the user's
    /// implementation returns an error is not, since the application already observes that error
    /// directly. Because a timeout cancels the in-flight handshake future, it is otherwise invisible
    /// to user code. A rising rate suggests peers that connect but stall without completing the
    /// handshake (e.g. a slowloris-style probe).
    pub fn handshake_timeouts(&self) -> u64 {
        self.handshake_timeouts.load(Relaxed)
    }

    /// Registers a single connection dropped due to a read idle timeout.
    pub(crate) fn register_idle_timeout(&self) {
        self.idle_timeouts.fetch_add(1, Relaxed);
    }

    /// Returns the number of connections dropped because no inbound message arrived within
    /// [`Reading::IDLE_TIMEOUT_MS`](Reading::IDLE_TIMEOUT_MS).
    ///
    /// note: This isolates idle timeouts from the other causes of a reader-side disconnect (a decode
    /// error or the peer closing its end), which all otherwise surface indistinguishably as
    /// [`DisconnectOrigin::Reading`](DisconnectOrigin::Reading). A rising rate points to peers
    /// that hold a connection open without sending anything.
    pub fn idle_timeouts(&self) -> u64 {
        self.idle_timeouts.load(Relaxed)
    }

    /// Registers a single connection dropped due to a write timeout.
    pub(crate) fn register_write_timeout(&self) {
        self.write_timeouts.fetch_add(1, Relaxed);
    }

    /// Returns the number of connections dropped because an outbound write did not flush within
    /// [`Writing::TIMEOUT_MS`](Writing::TIMEOUT_MS).
    ///
    /// note: This isolates write timeouts from other causes of a writer-side disconnect (an
    /// underlying socket error or the channel closing), which all otherwise surface indistinguishably
    /// as [`DisconnectOrigin::Writing`](DisconnectOrigin::Writing). A rising rate points to
    /// peers that stop reading (a stalled or saturated receiver), leaving the local send buffer full.
    pub fn write_timeouts(&self) -> u64 {
        self.write_timeouts.load(Relaxed)
    }
}
