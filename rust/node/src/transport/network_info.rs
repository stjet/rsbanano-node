use super::{ChannelDirection, ChannelId, ChannelMode, TrafficType};
use num::FromPrimitive;
use rand::{seq::SliceRandom, thread_rng};
use rsnano_core::{
    utils::{seconds_since_epoch, TEST_ENDPOINT_1, TEST_ENDPOINT_2},
    PublicKey,
};
use rsnano_messages::ProtocolInfo;
use std::{
    collections::HashMap,
    net::SocketAddrV6,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Default timeout in seconds
const DEFAULT_TIMEOUT: u64 = 120;

pub struct ChannelInfo {
    channel_id: ChannelId,
    local_addr: SocketAddrV6,
    peer_addr: SocketAddrV6,
    data: Mutex<ChannelInfoData>,
    protocol_version: AtomicU8,
    direction: ChannelDirection,

    /// the timestamp (in seconds since epoch) of the last time there was successful activity on the socket
    last_activity: AtomicU64, // TODO use Timestamp

    /// Duration in seconds of inactivity that causes a socket timeout
    /// activity is any successful connect, send or receive event
    timeout_seconds: AtomicU64,

    /// Flag that is set when cleanup decides to close the socket due to timeout.
    /// NOTE: Currently used by tcp_server::timeout() but I suspect that this and tcp_server::timeout() are not needed.
    timed_out: AtomicBool,

    /// Set by close() - completion handlers must check this. This is more reliable than checking
    /// error codes as the OS may have already completed the async operation.
    closed: AtomicBool,

    socket_type: AtomicU8,
}

impl ChannelInfo {
    pub fn new(
        channel_id: ChannelId,
        local_addr: SocketAddrV6,
        peer_addr: SocketAddrV6,
        direction: ChannelDirection,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            channel_id,
            local_addr,
            peer_addr,
            // TODO set protocol version to 0
            protocol_version: AtomicU8::new(ProtocolInfo::default().version_using),
            direction,
            last_activity: AtomicU64::new(seconds_since_epoch()),
            timeout_seconds: AtomicU64::new(DEFAULT_TIMEOUT),
            timed_out: AtomicBool::new(false),
            socket_type: AtomicU8::new(ChannelMode::Undefined as u8),
            closed: AtomicBool::new(false),
            data: Mutex::new(ChannelInfoData {
                node_id: None,
                last_bootstrap_attempt: UNIX_EPOCH,
                last_packet_received: now,
                last_packet_sent: now,
                is_queue_full_impl: None,
                peering_addr: if direction == ChannelDirection::Outbound {
                    Some(peer_addr)
                } else {
                    None
                },
            }),
        }
    }

    pub fn new_test_instance() -> Self {
        Self::new(
            ChannelId::from(42),
            TEST_ENDPOINT_1,
            TEST_ENDPOINT_2,
            ChannelDirection::Outbound,
        )
    }

    pub fn set_queue_full_query(&self, query: Box<dyn Fn(TrafficType) -> bool + Send>) {
        self.data.lock().unwrap().is_queue_full_impl = Some(query);
    }

    pub fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    pub fn node_id(&self) -> Option<PublicKey> {
        self.data.lock().unwrap().node_id
    }

    pub fn direction(&self) -> ChannelDirection {
        self.direction
    }

    pub fn local_addr(&self) -> SocketAddrV6 {
        self.local_addr
    }

    /// The address that we are connected to. If this is an incoming channel, then
    /// the peer_addr uses an ephemeral port
    pub fn peer_addr(&self) -> SocketAddrV6 {
        self.peer_addr
    }

    /// The address where the peer accepts incoming connections. In case of an outbound
    /// channel, the peer_addr and peering_addr are the same
    pub fn peering_addr(&self) -> Option<SocketAddrV6> {
        self.data.lock().unwrap().peering_addr.clone()
    }

    pub fn peering_addr_or_peer_addr(&self) -> SocketAddrV6 {
        self.data
            .lock()
            .unwrap()
            .peering_addr
            .clone()
            .unwrap_or(self.peer_addr())
    }

    pub fn protocol_version(&self) -> u8 {
        self.protocol_version.load(Ordering::Relaxed)
    }

    // TODO make private and set via NetworkInfo
    pub fn set_protocol_version(&self, version: u8) {
        self.protocol_version.store(version, Ordering::Relaxed);
    }

    pub fn last_activity(&self) -> u64 {
        self.last_activity.load(Ordering::Relaxed)
    }

    pub fn set_last_activity(&self, value: u64) {
        self.last_activity.store(value, Ordering::Relaxed)
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds.load(Ordering::Relaxed))
    }

    pub fn set_timeout(&self, value: Duration) {
        self.timeout_seconds
            .store(value.as_secs(), Ordering::Relaxed)
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Relaxed)
    }

    pub fn set_timed_out(&self, value: bool) {
        self.timed_out.store(value, Ordering::Relaxed)
    }

    pub fn is_alive(&self) -> bool {
        !self.is_closed()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.set_timeout(Duration::ZERO);
    }

    pub fn set_node_id(&self, node_id: PublicKey) {
        self.data.lock().unwrap().node_id = Some(node_id);
    }

    pub fn set_peering_addr(&self, peering_addr: SocketAddrV6) {
        self.data.lock().unwrap().peering_addr = Some(peering_addr);
    }

    pub fn mode(&self) -> ChannelMode {
        FromPrimitive::from_u8(self.socket_type.load(Ordering::SeqCst)).unwrap()
    }

    pub fn set_mode(&self, mode: ChannelMode) {
        self.socket_type.store(mode as u8, Ordering::SeqCst);
    }

    pub fn last_bootstrap_attempt(&self) -> SystemTime {
        self.data.lock().unwrap().last_bootstrap_attempt
    }

    pub fn set_last_bootstrap_attempt(&self, time: SystemTime) {
        self.data.lock().unwrap().last_bootstrap_attempt = time;
    }

    pub fn last_packet_received(&self) -> SystemTime {
        self.data.lock().unwrap().last_packet_received
    }

    pub fn set_last_packet_received(&self, instant: SystemTime) {
        self.data.lock().unwrap().last_packet_received = instant;
    }

    pub fn last_packet_sent(&self) -> SystemTime {
        self.data.lock().unwrap().last_packet_sent
    }

    pub fn set_last_packet_sent(&self, instant: SystemTime) {
        self.data.lock().unwrap().last_packet_sent = instant;
    }

    pub fn is_queue_full(&self, traffic_type: TrafficType) -> bool {
        let guard = self.data.lock().unwrap();
        match &guard.is_queue_full_impl {
            Some(cb) => cb(traffic_type),
            None => false,
        }
    }
}

struct ChannelInfoData {
    node_id: Option<PublicKey>,
    peering_addr: Option<SocketAddrV6>,
    last_bootstrap_attempt: SystemTime,
    last_packet_received: SystemTime,
    last_packet_sent: SystemTime,
    is_queue_full_impl: Option<Box<dyn Fn(TrafficType) -> bool + Send>>,
}

pub struct NetworkInfo {
    next_channel_id: usize,
    channels: HashMap<ChannelId, Arc<ChannelInfo>>,
    listening_port: u16,
    stopped: bool,
    new_realtime_channel_observers: Vec<Arc<dyn Fn(Arc<ChannelInfo>) + Send + Sync>>,
}

impl NetworkInfo {
    pub fn new(listening_port: u16) -> Self {
        Self {
            next_channel_id: 1,
            channels: HashMap::new(),
            listening_port,
            stopped: false,
            new_realtime_channel_observers: Vec::new(),
        }
    }

    pub(crate) fn on_new_realtime_channel(
        &mut self,
        callback: Arc<dyn Fn(Arc<ChannelInfo>) + Send + Sync>,
    ) {
        self.new_realtime_channel_observers.push(callback);
    }

    pub(crate) fn new_realtime_channel_observers(
        &self,
    ) -> Vec<Arc<dyn Fn(Arc<ChannelInfo>) + Send + Sync>> {
        self.new_realtime_channel_observers.clone()
    }

    pub fn add(
        &mut self,
        local_addr: SocketAddrV6,
        peer_addr: SocketAddrV6,
        direction: ChannelDirection,
    ) -> Arc<ChannelInfo> {
        let channel_id = self.get_next_channel_id();
        let channel_info = Arc::new(ChannelInfo::new(
            channel_id, local_addr, peer_addr, direction,
        ));
        self.channels.insert(channel_id, channel_info.clone());
        channel_info
    }

    fn get_next_channel_id(&mut self) -> ChannelId {
        let id = self.next_channel_id.into();
        self.next_channel_id += 1;
        id
    }

    pub fn listening_port(&self) -> u16 {
        self.listening_port
    }

    pub fn set_listening_port(&mut self, port: u16) {
        self.listening_port = port
    }

    pub fn get(&self, channel_id: ChannelId) -> Option<&Arc<ChannelInfo>> {
        self.channels.get(&channel_id)
    }

    pub fn remove(&mut self, channel_id: ChannelId) {
        self.channels.remove(&channel_id);
    }

    pub fn set_node_id(&self, channel_id: ChannelId, node_id: PublicKey) {
        if let Some(channel) = self.channels.get(&channel_id) {
            channel.set_node_id(node_id);
        }
    }

    pub fn find_node_id(&self, node_id: &PublicKey) -> Option<&Arc<ChannelInfo>> {
        self.channels
            .values()
            .find(|c| c.node_id() == Some(*node_id))
    }

    pub fn random_realtime_channels(&self, count: usize, min_version: u8) -> Vec<Arc<ChannelInfo>> {
        let mut channels = self.list_realtime(min_version);
        let mut rng = thread_rng();
        channels.shuffle(&mut rng);
        if count > 0 {
            channels.truncate(count)
        }
        channels
    }

    pub fn list_realtime(&self, min_version: u8) -> Vec<Arc<ChannelInfo>> {
        self.channels
            .values()
            .filter(|c| {
                c.protocol_version() >= min_version
                    && c.is_alive()
                    && c.mode() == ChannelMode::Realtime
            })
            .map(|c| c.clone())
            .collect()
    }

    pub(crate) fn list_realtime_channels(&self, min_version: u8) -> Vec<Arc<ChannelInfo>> {
        let mut result = self.list_realtime(min_version);
        result.sort_by_key(|i| i.peer_addr());
        result
    }

    pub fn stop(&mut self) -> bool {
        if self.stopped {
            false
        } else {
            self.stopped = true;
            true
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }
}
