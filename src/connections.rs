use parking_lot::RwLock;
use tokio::sync::Mutex;

use crate::connection::Connection;

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

type LockedConnectionMap = RwLock<HashMap<SocketAddr, Arc<Mutex<Connection>>>>;

#[derive(Default)]
pub(crate) struct Connections {
    pub(crate) handshaking: LockedConnectionMap,
    pub(crate) handshaken: LockedConnectionMap,
}
