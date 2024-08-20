use super::{
    channel_container::ChannelContainer, Channel, ChannelDirection, ChannelId, ChannelMode,
    DropPolicy, NetworkFilter, NetworkInfo, OutboundBandwidthLimiter, TcpConfig, TcpStream,
    TrafficType,
};
use crate::{
    config::{NetworkConstants, NodeFlags},
    stats::{DetailType, Direction, StatType, Stats},
    utils::{
        into_ipv6_socket_address, ipv4_address_or_ipv6_subnet, map_address_to_subnetwork,
        SteadyClock,
    },
    NetworkParams, DEV_NETWORK_PARAMS,
};
use rand::{seq::SliceRandom, thread_rng};
use rsnano_core::{utils::NULL_ENDPOINT, Account};
use rsnano_messages::*;
use std::{
    net::{Ipv6Addr, SocketAddrV6},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant, SystemTime},
};
use tracing::{debug, warn};

pub struct NetworkOptions {
    pub allow_local_peers: bool,
    pub tcp_config: TcpConfig,
    pub publish_filter: Arc<NetworkFilter>,
    pub network_params: NetworkParams,
    pub stats: Arc<Stats>,
    pub flags: NodeFlags,
    pub limiter: Arc<OutboundBandwidthLimiter>,
    pub clock: Arc<SteadyClock>,
    pub network_info: Arc<RwLock<NetworkInfo>>,
}

impl NetworkOptions {
    pub fn new_test_instance() -> Self {
        NetworkOptions {
            allow_local_peers: true,
            tcp_config: TcpConfig::for_dev_network(),
            publish_filter: Arc::new(NetworkFilter::default()),
            network_params: DEV_NETWORK_PARAMS.clone(),
            stats: Arc::new(Default::default()),
            flags: NodeFlags::default(),
            limiter: Arc::new(OutboundBandwidthLimiter::default()),
            clock: Arc::new(SteadyClock::new_null()),
            network_info: Arc::new(RwLock::new(NetworkInfo::new_test_instance())),
        }
    }
}

pub struct Network {
    state: Mutex<State>,
    pub info: Arc<RwLock<NetworkInfo>>,
    allow_local_peers: bool,
    flags: NodeFlags,
    stats: Arc<Stats>,
    network_params: Arc<NetworkParams>,
    limiter: Arc<OutboundBandwidthLimiter>,
    tcp_config: TcpConfig,
    pub publish_filter: Arc<NetworkFilter>,
    clock: Arc<SteadyClock>,
}

impl Drop for Network {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Network {
    pub fn new(options: NetworkOptions) -> Self {
        let network = Arc::new(options.network_params);

        Self {
            allow_local_peers: options.allow_local_peers,
            state: Mutex::new(State {
                channels: Default::default(),
                network_constants: network.network.clone(),
            }),
            tcp_config: options.tcp_config,
            flags: options.flags,
            stats: options.stats,
            network_params: network,
            limiter: options.limiter,
            publish_filter: options.publish_filter,
            clock: options.clock,
            info: options.network_info,
        }
    }

    pub(crate) fn channels_info(&self) -> ChannelsInfo {
        self.state.lock().unwrap().channels_info()
    }

    pub(crate) async fn wait_for_available_inbound_slot(&self) {
        let last_log = Instant::now();
        let log_interval = if self.network_params.network.is_dev_network() {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(15)
        };
        while self.count_by_direction(ChannelDirection::Inbound)
            >= self.tcp_config.max_inbound_connections
            && !self.info.read().unwrap().is_stopped()
        {
            if last_log.elapsed() >= log_interval {
                warn!(
                    "Waiting for available slots to accept new connections (current: {} / max: {})",
                    self.count_by_direction(ChannelDirection::Inbound),
                    self.tcp_config.max_inbound_connections
                );
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn can_add_connection(
        &self,
        peer_addr: &SocketAddrV6,
        direction: ChannelDirection,
        planned_mode: ChannelMode,
    ) -> AcceptResult {
        if self
            .info
            .write()
            .unwrap()
            .excluded_peers
            .is_excluded(peer_addr, self.clock.now())
        {
            return AcceptResult::Rejected;
        }
        if direction == ChannelDirection::Outbound {
            if self.can_add_outbound_connection(&peer_addr, planned_mode) {
                AcceptResult::Accepted
            } else {
                AcceptResult::Rejected
            }
        } else {
            self.check_limits(&peer_addr, direction)
        }
    }

    pub async fn add(
        &self,
        stream: TcpStream,
        direction: ChannelDirection,
        planned_mode: ChannelMode,
    ) -> anyhow::Result<Arc<Channel>> {
        let peer_addr = stream
            .peer_addr()
            .map(into_ipv6_socket_address)
            .unwrap_or(NULL_ENDPOINT);

        let local_addr = stream
            .local_addr()
            .map(into_ipv6_socket_address)
            .unwrap_or(NULL_ENDPOINT);

        let result = self.can_add_connection(&peer_addr, direction, planned_mode);
        if result != AcceptResult::Accepted {
            self.stats.inc_dir(
                StatType::TcpListener,
                DetailType::AcceptRejected,
                direction.into(),
            );
            if direction == ChannelDirection::Outbound {
                self.stats.inc_dir(
                    StatType::TcpListener,
                    DetailType::ConnectFailure,
                    Direction::Out,
                );
            }
            debug!(?peer_addr, ?direction, "Rejected connection");
            if direction == ChannelDirection::Inbound {
                self.stats.inc_dir(
                    StatType::TcpListener,
                    DetailType::AcceptFailure,
                    Direction::In,
                );
                // Refusal reason should be logged earlier
            }
            return Err(anyhow!("check_limits failed"));
        }

        self.stats.inc_dir(
            StatType::TcpListener,
            DetailType::AcceptSuccess,
            direction.into(),
        );

        if direction == ChannelDirection::Outbound {
            self.stats.inc_dir(
                StatType::TcpListener,
                DetailType::ConnectSuccess,
                Direction::Out,
            );
        }

        let channel_info = self
            .info
            .write()
            .unwrap()
            .add(local_addr, peer_addr, direction);

        let channel = Channel::create(
            channel_info,
            stream,
            self.stats.clone(),
            self.limiter.clone(),
            self.info.clone(),
        )
        .await;
        self.state.lock().unwrap().channels.insert(channel.clone());

        debug!(?peer_addr, ?direction, "Accepted connection");

        Ok(channel)
    }

    pub(crate) fn new_null() -> Self {
        Self::new(NetworkOptions::new_test_instance())
    }

    pub(crate) fn stop(&self) {
        if self.info.write().unwrap().stop() {
            self.close();
        }
    }

    fn close(&self) {
        self.state.lock().unwrap().close_channels();
    }

    pub(crate) fn check_limits(
        &self,
        ip: &SocketAddrV6,
        direction: ChannelDirection,
    ) -> AcceptResult {
        self.info.write().unwrap().check_limits(ip, direction)
    }

    pub(crate) fn add_attempt(&self, remote: SocketAddrV6) -> bool {
        self.info.write().unwrap().add_attempt(remote)
    }

    pub(crate) fn remove_attempt(&self, remote: &SocketAddrV6) {
        self.info.write().unwrap().remove_attempt(remote)
    }

    pub fn random_fill_peering_endpoints(&self, endpoints: &mut [SocketAddrV6]) {
        self.state.lock().unwrap().random_fill_realtime(endpoints);
    }

    pub fn random_fanout_realtime(&self, scale: f32) -> Vec<Arc<Channel>> {
        self.state.lock().unwrap().random_fanout_realtime(scale)
    }

    pub(crate) fn is_queue_full(&self, channel_id: ChannelId, traffic_type: TrafficType) -> bool {
        self.state
            .lock()
            .unwrap()
            .channels
            .get_by_id(channel_id)
            .map(|c| c.info.is_queue_full(traffic_type))
            .unwrap_or(true)
    }

    pub(crate) fn try_send_buffer(
        &self,
        channel_id: ChannelId,
        buffer: &[u8],
        drop_policy: DropPolicy,
        traffic_type: TrafficType,
    ) -> bool {
        if let Some(channel) = self.state.lock().unwrap().channels.get_by_id(channel_id) {
            channel.try_send_buffer(buffer, drop_policy, traffic_type)
        } else {
            false
        }
    }

    pub async fn send_buffer(
        &self,
        channel_id: ChannelId,
        buffer: &[u8],
        traffic_type: TrafficType,
    ) -> anyhow::Result<()> {
        let channel = self
            .state
            .lock()
            .unwrap()
            .channels
            .get_by_id(channel_id)
            .cloned();

        if let Some(channel) = channel {
            channel.send_buffer(buffer, traffic_type).await
        } else {
            Err(anyhow!("Channel not found"))
        }
    }

    fn max_ip_or_subnetwork_connections(&self, endpoint: &SocketAddrV6) -> bool {
        self.max_ip_connections(endpoint) || self.max_subnetwork_connections(endpoint)
    }

    fn max_ip_connections(&self, endpoint: &SocketAddrV6) -> bool {
        if self.flags.disable_max_peers_per_ip {
            return false;
        }
        let mut result;
        let address = ipv4_address_or_ipv6_subnet(endpoint.ip());
        let lock = self.state.lock().unwrap();
        result = lock.channels.count_by_ip(&address) >= lock.network_constants.max_peers_per_ip;
        if !result {
            result = self
                .info
                .read()
                .unwrap()
                .attempts
                .count_by_address(&address)
                >= lock.network_constants.max_peers_per_ip;
        }
        if result {
            self.stats
                .inc_dir(StatType::Tcp, DetailType::MaxPerIp, Direction::Out);
        }
        result
    }

    fn max_subnetwork_connections(&self, endoint: &SocketAddrV6) -> bool {
        if self.flags.disable_max_peers_per_subnetwork {
            return false;
        }

        let subnet = map_address_to_subnetwork(endoint.ip());
        let is_max = {
            let guard = self.state.lock().unwrap();
            guard.channels.count_by_subnet(&subnet)
                >= self.network_params.network.max_peers_per_subnetwork
                || self
                    .info
                    .read()
                    .unwrap()
                    .attempts
                    .count_by_subnetwork(&subnet)
                    >= self.network_params.network.max_peers_per_subnetwork
        };

        if is_max {
            self.stats
                .inc_dir(StatType::Tcp, DetailType::MaxPerSubnetwork, Direction::Out);
        }

        is_max
    }

    fn can_add_outbound_connection(&self, peer: &SocketAddrV6, planned_mode: ChannelMode) -> bool {
        if self.flags.disable_tcp_realtime {
            return false;
        }

        // Don't contact invalid IPs
        if self
            .info
            .read()
            .unwrap()
            .not_a_peer(peer, self.allow_local_peers)
        {
            return false;
        }

        // Don't overload single IP
        if self.max_ip_or_subnetwork_connections(peer) {
            return false;
        }

        if self
            .info
            .write()
            .unwrap()
            .excluded_peers
            .is_excluded(peer, self.clock.now())
        {
            return false;
        }

        let state = self.state.lock().unwrap();
        // Don't connect to nodes that already sent us something
        if state
            .find_channels_by_remote_addr(peer)
            .iter()
            .any(|c| c.info.mode() == planned_mode || c.info.mode() == ChannelMode::Undefined)
        {
            return false;
        }
        if state
            .find_channels_by_peering_addr(peer)
            .iter()
            .any(|c| c.info.mode() == planned_mode || c.info.mode() == ChannelMode::Undefined)
        {
            return false;
        }

        if self
            .info
            .write()
            .unwrap()
            .check_limits(peer, ChannelDirection::Outbound)
            != AcceptResult::Accepted
        {
            self.stats.inc_dir(
                StatType::TcpListener,
                DetailType::ConnectRejected,
                Direction::Out,
            );
            // Refusal reason should be logged earlier

            return false; // Rejected
        }

        self.stats.inc_dir(
            StatType::TcpListener,
            DetailType::ConnectInitiate,
            Direction::Out,
        );
        debug!("Initiate outgoing connection to: {}", peer);

        true
    }

    pub fn len_sqrt(&self) -> f32 {
        self.state.lock().unwrap().len_sqrt()
    }
    /// Desired fanout for a given scale
    /// Simulating with sqrt_broadcast_simulate shows we only need to broadcast to sqrt(total_peers) random peers in order to successfully publish to everyone with high probability
    pub fn fanout(&self, scale: f32) -> usize {
        self.state.lock().unwrap().fanout(scale)
    }

    /// Returns channel IDs of removed channels
    pub fn purge(&self, cutoff: SystemTime) -> Vec<ChannelId> {
        let channel_ids = self.info.write().unwrap().purge(cutoff);
        let mut guard = self.state.lock().unwrap();
        for channel_id in &channel_ids {
            guard.channels.remove_by_id(*channel_id);
        }
        channel_ids
    }

    pub fn count_by_mode(&self, mode: ChannelMode) -> usize {
        self.state.lock().unwrap().channels.count_by_mode(mode)
    }

    pub(crate) fn count_by_direction(&self, direction: ChannelDirection) -> usize {
        self.state
            .lock()
            .unwrap()
            .channels
            .count_by_direction(direction)
    }

    pub(crate) fn bootstrap_peer(&self) -> SocketAddrV6 {
        self.state.lock().unwrap().bootstrap_peer()
    }

    pub fn port(&self) -> u16 {
        self.info.read().unwrap().listening_port()
    }

    pub(crate) fn set_port(&self, port: u16) {
        self.info.write().unwrap().set_listening_port(port);
    }

    pub(crate) fn set_peering_addr(&self, channel_id: ChannelId, peering_addr: SocketAddrV6) {
        let guard = self.state.lock().unwrap();
        if let Some(channel) = guard.channels.get_by_id(channel_id) {
            channel.info.set_peering_addr(peering_addr);
        }
    }

    pub(crate) fn create_keepalive_message(&self) -> Message {
        let mut peers = [SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0); 8];
        self.random_fill_peering_endpoints(&mut peers);
        Message::Keepalive(Keepalive { peers })
    }

    pub(crate) fn is_excluded(&self, addr: &SocketAddrV6) -> bool {
        self.info
            .write()
            .unwrap()
            .excluded_peers
            .is_excluded(addr, self.clock.now())
    }

    pub(crate) fn peer_misbehaved(&self, channel_id: ChannelId) {
        let guard = self.state.lock().unwrap();

        let Some(channel) = guard.channels.get_by_id(channel_id) else {
            return;
        };
        let channel = channel.clone();

        // Add to peer exclusion list

        self.info
            .write()
            .unwrap()
            .excluded_peers
            .peer_misbehaved(&channel.info.peer_addr(), self.clock.now());

        let peer_addr = channel.info.peer_addr();
        let mode = channel.info.mode();
        let direction = channel.info.direction();

        channel.info.close();
        drop(guard);

        warn!(?peer_addr, ?mode, ?direction, "Peer misbehaved!");
    }

    pub(crate) fn perma_ban(&self, remote_addr: SocketAddrV6) {
        self.info
            .write()
            .unwrap()
            .excluded_peers
            .perma_ban(remote_addr);
    }

    pub(crate) fn set_protocol_version(&self, channel_id: ChannelId, protocol_version: u8) {
        self.state
            .lock()
            .unwrap()
            .channels
            .set_protocol_version(channel_id, protocol_version);
    }

    pub(crate) fn upgrade_to_realtime_connection(
        &self,
        channel_id: ChannelId,
        node_id: Account,
    ) -> bool {
        let (observers, channel) = {
            let state = self.state.lock().unwrap();

            if self.info.read().unwrap().is_stopped() {
                return false;
            }

            let Some(channel) = state.channels.get_by_id(channel_id) else {
                return false;
            };

            if let Some(other) = state.channels.get_by_node_id(&node_id) {
                if other.ipv4_address_or_ipv6_subnet() == channel.ipv4_address_or_ipv6_subnet() {
                    // We already have a connection to that node. We allow duplicate node ids, but
                    // only if they come from different IP addresses
                    let endpoint = channel.info.peer_addr();
                    debug!(
                        node_id = node_id.to_node_id(),
                        remote = %endpoint,
                        "Could not upgrade channel {} to realtime connection, because another channel for the same node ID was found",
                        channel.channel_id(),
                    );
                    drop(state);
                    return false;
                }
            }

            channel.info.set_node_id(node_id);
            channel.info.set_mode(ChannelMode::Realtime);

            let observers = self.info.read().unwrap().new_realtime_channel_observers();
            let channel = channel.clone();
            (observers, channel)
        };

        self.stats
            .inc(StatType::TcpChannels, DetailType::ChannelAccepted);

        debug!(
            "Switched to realtime mode (addr: {}, node_id: {})",
            channel.info.peer_addr(),
            node_id.to_node_id()
        );

        for observer in observers {
            observer(channel.info.clone());
        }

        true
    }

    pub(crate) fn keepalive_list(&self) -> Vec<ChannelId> {
        let guard = self.state.lock().unwrap();
        guard.keepalive_list()
    }
}

struct State {
    channels: ChannelContainer,
    network_constants: NetworkConstants,
}

impl State {
    pub fn bootstrap_peer(&mut self) -> SocketAddrV6 {
        let mut peering_endpoint = None;
        let mut channel_id = None;
        for channel in self.channels.iter_by_last_bootstrap_attempt() {
            if channel.info.mode() == ChannelMode::Realtime
                && channel.info.protocol_version() >= self.network_constants.protocol_version_min
            {
                if let Some(peering) = channel.info.peering_addr() {
                    channel_id = Some(channel.channel_id());
                    peering_endpoint = Some(peering);
                    break;
                }
            }
        }

        match (channel_id, peering_endpoint) {
            (Some(id), Some(peering)) => {
                self.channels
                    .set_last_bootstrap_attempt(id, SystemTime::now());
                peering
            }
            _ => SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0),
        }
    }

    pub fn close_channels(&mut self) {
        for channel in self.channels.iter() {
            channel.info.close();
        }
        self.channels.clear();
    }

    pub fn random_realtime_channels(&self, count: usize, min_version: u8) -> Vec<Arc<Channel>> {
        let mut channels = self.list_realtime(min_version);
        let mut rng = thread_rng();
        channels.shuffle(&mut rng);
        if count > 0 {
            channels.truncate(count)
        }
        channels
    }

    pub fn list_realtime(&self, min_version: u8) -> Vec<Arc<Channel>> {
        self.channels
            .iter()
            .filter(|c| {
                c.info.protocol_version() >= min_version
                    && c.info.is_alive()
                    && c.info.mode() == ChannelMode::Realtime
            })
            .map(|c| c.clone())
            .collect()
    }

    pub fn keepalive_list(&self) -> Vec<ChannelId> {
        let cutoff = SystemTime::now() - self.network_constants.keepalive_period;
        let mut result = Vec::new();
        for channel in self.channels.iter() {
            if channel.info.mode() == ChannelMode::Realtime
                && channel.info.last_packet_sent() < cutoff
            {
                result.push(channel.channel_id());
            }
        }

        result
    }

    pub(crate) fn find_channels_by_remote_addr(
        &self,
        remote_addr: &SocketAddrV6,
    ) -> Vec<Arc<Channel>> {
        self.channels
            .get_by_remote_addr(remote_addr)
            .iter()
            .filter(|c| c.info.is_alive())
            .map(|&c| c.clone())
            .collect()
    }

    pub(crate) fn find_channels_by_peering_addr(
        &self,
        peering_addr: &SocketAddrV6,
    ) -> Vec<Arc<Channel>> {
        self.channels
            .get_by_peering_addr(peering_addr)
            .iter()
            .filter(|c| c.info.is_alive())
            .map(|&c| c.clone())
            .collect()
    }

    pub fn random_fanout_realtime(&self, scale: f32) -> Vec<Arc<Channel>> {
        self.random_realtime_channels(self.fanout(scale), 0)
    }

    pub fn random_fill_realtime(&self, endpoints: &mut [SocketAddrV6]) {
        let mut peers = self.list_realtime(0);
        // Don't include channels with ephemeral remote ports
        peers.retain(|c| c.info.peering_addr().is_some());
        let mut rng = thread_rng();
        peers.shuffle(&mut rng);
        peers.truncate(endpoints.len());

        let null_endpoint = SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0);

        for (i, target) in endpoints.iter_mut().enumerate() {
            let endpoint = if i < peers.len() {
                peers[i].info.peering_addr().unwrap_or(null_endpoint)
            } else {
                null_endpoint
            };
            *target = endpoint;
        }
    }

    pub fn len_sqrt(&self) -> f32 {
        f32::sqrt(self.channels.count_by_mode(ChannelMode::Realtime) as f32)
    }

    pub fn fanout(&self, scale: f32) -> usize {
        (self.len_sqrt() * scale).ceil() as usize
    }

    pub fn channels_info(&self) -> ChannelsInfo {
        let mut info = ChannelsInfo::default();
        for channel in self.channels.iter() {
            info.total += 1;
            match channel.info.mode() {
                ChannelMode::Bootstrap => info.bootstrap += 1,
                ChannelMode::Realtime => info.realtime += 1,
                _ => {}
            }
            match channel.info.direction() {
                ChannelDirection::Inbound => info.inbound += 1,
                ChannelDirection::Outbound => info.outbound += 1,
            }
        }
        info
    }
}

#[derive(PartialEq, Eq)]
pub enum AcceptResult {
    Invalid,
    Accepted,
    Rejected,
    Error,
}

#[derive(Default)]
pub(crate) struct ChannelsInfo {
    pub total: usize,
    pub realtime: usize,
    pub bootstrap: usize,
    pub inbound: usize,
    pub outbound: usize,
}

#[cfg(test)]
mod tests {
    use rsnano_core::{
        utils::{TEST_ENDPOINT_1, TEST_ENDPOINT_2, TEST_ENDPOINT_3},
        PublicKey,
    };

    use super::*;

    #[tokio::test]
    async fn newly_added_channel_is_not_a_realtime_channel() {
        let network = Network::new(NetworkOptions::new_test_instance());
        network
            .add(
                TcpStream::new_null(),
                ChannelDirection::Inbound,
                ChannelMode::Realtime,
            )
            .await
            .unwrap();
        assert_eq!(
            network.info.read().unwrap().list_realtime_channels(0).len(),
            0
        );
    }

    #[tokio::test]
    async fn upgrade_channel_to_realtime_channel() {
        let network = Network::new(NetworkOptions::new_test_instance());
        let channel = network
            .add(
                TcpStream::new_null(),
                ChannelDirection::Inbound,
                ChannelMode::Realtime,
            )
            .await
            .unwrap();

        assert!(network.upgrade_to_realtime_connection(channel.channel_id(), PublicKey::from(456)));
        assert_eq!(
            network.info.read().unwrap().list_realtime_channels(0).len(),
            1
        );
    }

    #[test]
    fn random_fill_peering_endpoints_empty() {
        let network = Network::new(NetworkOptions::new_test_instance());
        let mut endpoints = [NULL_ENDPOINT; 3];
        network.random_fill_peering_endpoints(&mut endpoints);
        assert_eq!(endpoints, [NULL_ENDPOINT; 3]);
    }

    #[tokio::test]
    async fn random_fill_peering_endpoints_part() {
        let network = Network::new(NetworkOptions::new_test_instance());
        add_realtime_channel_with_peering_addr(&network, TEST_ENDPOINT_1).await;
        add_realtime_channel_with_peering_addr(&network, TEST_ENDPOINT_2).await;
        let mut endpoints = [NULL_ENDPOINT; 3];
        network.random_fill_peering_endpoints(&mut endpoints);
        assert!(endpoints.contains(&TEST_ENDPOINT_1));
        assert!(endpoints.contains(&TEST_ENDPOINT_2));
        assert_eq!(endpoints[2], NULL_ENDPOINT);
    }

    #[tokio::test]
    async fn random_fill_peering_endpoints() {
        let network = Network::new(NetworkOptions::new_test_instance());
        add_realtime_channel_with_peering_addr(&network, TEST_ENDPOINT_1).await;
        add_realtime_channel_with_peering_addr(&network, TEST_ENDPOINT_2).await;
        add_realtime_channel_with_peering_addr(&network, TEST_ENDPOINT_3).await;
        let mut endpoints = [NULL_ENDPOINT; 3];
        network.random_fill_peering_endpoints(&mut endpoints);
        assert!(endpoints.contains(&TEST_ENDPOINT_1));
        assert!(endpoints.contains(&TEST_ENDPOINT_2));
        assert!(endpoints.contains(&TEST_ENDPOINT_3));
    }

    async fn add_realtime_channel_with_peering_addr(network: &Network, peering_addr: SocketAddrV6) {
        let channel = network
            .add(
                TcpStream::new_null_with_peer_addr(peering_addr),
                ChannelDirection::Inbound,
                ChannelMode::Realtime,
            )
            .await
            .unwrap();
        channel.info.set_peering_addr(peering_addr);
        network.upgrade_to_realtime_connection(
            channel.channel_id(),
            PublicKey::from(peering_addr.ip().to_bits()),
        );
    }
}
