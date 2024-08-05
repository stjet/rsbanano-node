use super::{
    AsyncBufferReader, BufferDropPolicy, Channel, ChannelDirection, ChannelId, ChannelMode,
    OutboundBandwidthLimiter, Socket, SocketBuilder, TcpStream, TrafficType,
};
use crate::{
    stats::{Direction, StatType, Stats},
    utils::{ipv4_address_or_ipv6_subnet, map_address_to_subnetwork},
};
use async_trait::async_trait;
use rsnano_core::Account;
use rsnano_messages::{Message, MessageSerializer, ProtocolInfo};
use std::{
    fmt::Display,
    net::{Ipv6Addr, SocketAddrV6},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::sleep;
use tracing::trace;

pub struct TcpChannelData {
    last_bootstrap_attempt: SystemTime,
    last_packet_received: SystemTime,
    last_packet_sent: SystemTime,
    node_id: Option<Account>,
    peering_endpoint: Option<SocketAddrV6>,
}

pub struct ChannelTcp {
    channel_id: ChannelId,
    channel_mutex: Mutex<TcpChannelData>,
    socket: Arc<Socket>,
    network_version: AtomicU8,
    limiter: Arc<OutboundBandwidthLimiter>,
    message_serializer: Mutex<MessageSerializer>, // TODO remove mutex
    stats: Arc<Stats>,
}

impl ChannelTcp {
    pub fn new(
        socket: Arc<Socket>,
        now: SystemTime,
        stats: Arc<Stats>,
        limiter: Arc<OutboundBandwidthLimiter>,
        channel_id: ChannelId,
        protocol: ProtocolInfo,
    ) -> Self {
        let peering_endpoint = match socket.direction() {
            ChannelDirection::Inbound => None,
            ChannelDirection::Outbound => Some(socket.remote_addr()),
        };
        Self {
            channel_id,
            channel_mutex: Mutex::new(TcpChannelData {
                last_bootstrap_attempt: UNIX_EPOCH,
                last_packet_received: now,
                last_packet_sent: now,
                node_id: None,
                peering_endpoint,
            }),
            socket,
            network_version: AtomicU8::new(protocol.version_using),
            limiter,
            message_serializer: Mutex::new(MessageSerializer::new(protocol)),
            stats,
        }
    }

    pub async fn create(
        channel_id: ChannelId,
        stream: TcpStream,
        direction: ChannelDirection,
        protocol: ProtocolInfo,
        stats: Arc<Stats>,
        limiter: Arc<OutboundBandwidthLimiter>,
    ) -> Self {
        let socket = SocketBuilder::new(direction, stream)
            .stats(stats.clone())
            .finish()
            .await;

        Self::new(
            socket,
            SystemTime::now(),
            stats,
            limiter,
            channel_id,
            protocol,
        )
    }

    pub(crate) fn set_peering_endpoint(&self, address: SocketAddrV6) {
        let mut lock = self.channel_mutex.lock().unwrap();
        lock.peering_endpoint = Some(address);
    }

    pub(crate) fn max(&self, traffic_type: TrafficType) -> bool {
        self.socket.max(traffic_type)
    }
}

impl Display for ChannelTcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.socket.remote_addr().fmt(f)
    }
}

#[async_trait]
impl Channel for Arc<ChannelTcp> {
    fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    fn get_last_bootstrap_attempt(&self) -> SystemTime {
        self.channel_mutex.lock().unwrap().last_bootstrap_attempt
    }

    fn set_last_bootstrap_attempt(&self, time: SystemTime) {
        self.channel_mutex.lock().unwrap().last_bootstrap_attempt = time;
    }

    fn get_last_packet_received(&self) -> SystemTime {
        self.channel_mutex.lock().unwrap().last_packet_received
    }

    fn set_last_packet_received(&self, instant: SystemTime) {
        self.channel_mutex.lock().unwrap().last_packet_received = instant;
    }

    fn get_last_packet_sent(&self) -> SystemTime {
        self.channel_mutex.lock().unwrap().last_packet_sent
    }

    fn set_last_packet_sent(&self, instant: SystemTime) {
        self.channel_mutex.lock().unwrap().last_packet_sent = instant;
    }

    fn get_node_id(&self) -> Option<Account> {
        self.channel_mutex.lock().unwrap().node_id
    }

    fn set_node_id(&self, id: Account) {
        self.channel_mutex.lock().unwrap().node_id = Some(id);
    }

    fn is_alive(&self) -> bool {
        self.socket.is_alive()
    }

    fn get_type(&self) -> super::TransportType {
        super::TransportType::Tcp
    }

    fn local_addr(&self) -> SocketAddrV6 {
        self.socket.local_endpoint_v6()
    }

    fn remote_addr(&self) -> SocketAddrV6 {
        self.socket.remote_addr()
    }

    fn peering_endpoint(&self) -> Option<SocketAddrV6> {
        self.channel_mutex.lock().unwrap().peering_endpoint
    }

    fn network_version(&self) -> u8 {
        self.network_version.load(Ordering::Relaxed)
    }

    fn direction(&self) -> ChannelDirection {
        self.socket.direction()
    }

    fn mode(&self) -> ChannelMode {
        self.socket.mode()
    }

    fn set_mode(&self, mode: ChannelMode) {
        self.socket.set_mode(mode)
    }

    fn set_timeout(&self, timeout: Duration) {
        self.socket.set_timeout(timeout);
    }

    fn try_send(
        &self,
        message: &Message,
        drop_policy: BufferDropPolicy,
        traffic_type: TrafficType,
    ) {
        let buffer = {
            let mut serializer = self.message_serializer.lock().unwrap();
            let buffer = serializer.serialize(message);
            Arc::new(Vec::from(buffer)) // TODO don't copy into vec. Pass slice directly
        };

        let is_droppable_by_limiter = drop_policy == BufferDropPolicy::Limiter;
        let should_pass = self.limiter.should_pass(buffer.len(), traffic_type.into());
        if !is_droppable_by_limiter || should_pass {
            self.socket.try_write(&buffer, traffic_type);
            self.stats
                .inc_dir_aggregate(StatType::Message, message.into(), Direction::Out);
            trace!(channel_id = %self.channel_id, message = ?message, "Message sent");
        } else {
            let detail_type = message.into();
            self.stats
                .inc_dir_aggregate(StatType::Drop, detail_type, Direction::Out);
            trace!(channel_id = %self.channel_id, message = ?message, "Message dropped");
        }
    }

    async fn send_buffer(&self, buffer: &[u8], traffic_type: TrafficType) -> anyhow::Result<()> {
        while !self.limiter.should_pass(buffer.len(), traffic_type.into()) {
            // TODO: better implementation
            sleep(Duration::from_millis(20)).await;
        }

        self.socket.write(buffer, traffic_type).await?;
        self.channel_mutex.lock().unwrap().last_packet_sent = SystemTime::now();
        Ok(())
    }

    async fn send(&self, message: &Message, traffic_type: TrafficType) -> anyhow::Result<()> {
        let buffer = {
            let mut serializer = self.message_serializer.lock().unwrap();
            let buffer = serializer.serialize(message);
            Arc::new(Vec::from(buffer)) // TODO don't copy into vec. Pass slice directly
        };
        self.send_buffer(&buffer, traffic_type).await?;
        self.stats
            .inc_dir_aggregate(StatType::Message, message.into(), Direction::Out);
        trace!(channel_id = %self.channel_id, message = ?message, "Message sent");
        Ok(())
    }

    fn close(&self) {
        self.socket.close();
    }

    fn ipv4_address_or_ipv6_subnet(&self) -> Ipv6Addr {
        ipv4_address_or_ipv6_subnet(&self.remote_addr().ip())
    }

    fn subnetwork(&self) -> Ipv6Addr {
        map_address_to_subnetwork(self.remote_addr().ip())
    }
}

impl Drop for ChannelTcp {
    fn drop(&mut self) {
        // Close socket. Exception: socket is used by bootstrap_server
        self.socket.close();
    }
}

impl PartialEq for ChannelTcp {
    fn eq(&self, other: &Self) -> bool {
        if Arc::as_ptr(&self.socket) != Arc::as_ptr(&other.socket) {
            return false;
        }

        true
    }
}

#[async_trait]
impl AsyncBufferReader for Arc<ChannelTcp> {
    async fn read(&self, buffer: &mut [u8], count: usize) -> anyhow::Result<()> {
        self.socket.read(buffer, count).await
    }
}
