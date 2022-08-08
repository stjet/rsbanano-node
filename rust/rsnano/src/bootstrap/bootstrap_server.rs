use std::{
    ffi::c_void,
    net::{Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use crate::{
    bootstrap::ParseStatus,
    logger_mt::Logger,
    messages::{Message, MessageVisitor},
    network::{Socket, SocketImpl, SocketType, TcpMessageItem, TcpMessageManager},
    stats::{DetailType, Direction, Stat, StatType},
    utils::{IoContext, ThreadPool},
    voting::VoteUniquer,
    Account, BlockUniquer, NetworkFilter, NetworkParams, NodeConfig, TelemetryCacheCutoffs,
};

use super::{MessageDeserializer, MessageDeserializerExt};

pub trait BootstrapServerObserver {
    fn bootstrap_server_timeout(&self, inner_ptr: usize);
    fn boostrap_server_exited(
        &self,
        socket_type: SocketType,
        unique_id: usize,
        endpoint: SocketAddr,
    );
    fn get_bootstrap_count(&self) -> usize;
    fn inc_bootstrap_count(&self);
    fn inc_realtime_count(&self);
}

pub struct BootstrapServer {
    pub socket: Arc<SocketImpl>,
    config: Arc<NodeConfig>,
    logger: Arc<dyn Logger>,
    stopped: AtomicBool,
    observer: Arc<dyn BootstrapServerObserver>,
    pub disable_bootstrap_listener: bool,
    pub connections_max: usize,

    // Remote enpoint used to remove response channel even after socket closing
    pub remote_endpoint: Mutex<SocketAddr>,
    pub remote_node_id: Mutex<Account>,
    workers: Arc<dyn ThreadPool>,
    io_ctx: Arc<dyn IoContext>,

    network: NetworkParams,
    last_telemetry_req: Mutex<Option<Instant>>,
    unique_id: usize,
    stats: Arc<Stat>,
    pub disable_bootstrap_bulk_pull_server: bool,
    pub disable_tcp_realtime: bool,
    pub handshake_query_received: AtomicBool,
    request_response_visitor_factory: Arc<dyn RequestResponseVisitorFactory>,
    message_deserializer: Arc<MessageDeserializer>,
    tcp_message_manager: Arc<TcpMessageManager>,
}

static NEXT_UNIQUE_ID: AtomicUsize = AtomicUsize::new(0);

impl BootstrapServer {
    pub fn new(
        socket: Arc<SocketImpl>,
        config: Arc<NodeConfig>,
        logger: Arc<dyn Logger>,
        observer: Arc<dyn BootstrapServerObserver>,
        publish_filter: Arc<NetworkFilter>,
        workers: Arc<dyn ThreadPool>,
        io_ctx: Arc<dyn IoContext>,
        network: NetworkParams,
        stats: Arc<Stat>,
        request_response_visitor_factory: Arc<dyn RequestResponseVisitorFactory>,
        block_uniquer: Arc<BlockUniquer>,
        vote_uniquer: Arc<VoteUniquer>,
        tcp_message_manager: Arc<TcpMessageManager>,
    ) -> Self {
        let network_constants = network.network.clone();
        Self {
            socket,
            config,
            logger,
            observer,
            stopped: AtomicBool::new(false),
            disable_bootstrap_listener: false,
            connections_max: 64,
            remote_endpoint: Mutex::new(SocketAddr::new(
                std::net::IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                0,
            )),
            remote_node_id: Mutex::new(Account::new()),
            workers,
            io_ctx,
            last_telemetry_req: Mutex::new(None),
            network,
            unique_id: NEXT_UNIQUE_ID.fetch_add(1, Ordering::Relaxed),
            stats,
            disable_bootstrap_bulk_pull_server: false,
            disable_tcp_realtime: false,
            handshake_query_received: AtomicBool::new(false),
            request_response_visitor_factory,
            message_deserializer: Arc::new(MessageDeserializer::new(
                network_constants,
                publish_filter,
                block_uniquer,
                vote_uniquer,
            )),
            tcp_message_manager,
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
            self.socket.close();
        }
    }

    pub fn is_telemetry_cutoff_exceeded(&self) -> bool {
        let cutoff = TelemetryCacheCutoffs::network_to_time(&self.network.network);
        let lock = self.last_telemetry_req.lock().unwrap();
        match *lock {
            Some(last_req) => last_req.elapsed() >= cutoff,
            None => true,
        }
    }

    pub fn to_bootstrap_connection(&self) -> bool {
        if self.socket.socket_type() == SocketType::Undefined
            && !self.disable_bootstrap_listener
            && self.observer.get_bootstrap_count() < self.connections_max
        {
            self.observer.inc_bootstrap_count();
            self.socket.set_socket_type(SocketType::Bootstrap);
        }

        return self.socket.socket_type() == SocketType::Bootstrap;
    }

    pub fn to_realtime_connection(&self, node_id: &Account) -> bool {
        if self.socket.socket_type() == SocketType::Undefined && !self.disable_tcp_realtime {
            {
                let mut lk = self.remote_node_id.lock().unwrap();
                *lk = *node_id;
            }

            self.observer.inc_realtime_count();
            self.socket.set_socket_type(SocketType::Realtime);
            return true;
        }
        return false;
    }

    pub fn set_last_telemetry_req(&self) {
        let mut lk = self.last_telemetry_req.lock().unwrap();
        *lk = Some(Instant::now());
    }

    pub fn cache_exceeded(&self) -> bool {
        let lk = self.last_telemetry_req.lock().unwrap();
        if let Some(last_req) = lk.as_ref() {
            last_req.elapsed() >= TelemetryCacheCutoffs::network_to_time(&self.network.network)
        } else {
            true
        }
    }

    pub fn unique_id(&self) -> usize {
        self.unique_id
    }

    pub fn is_undefined_connection(&self) -> bool {
        self.socket.socket_type() == SocketType::Undefined
    }

    pub fn is_bootstrap_connection(&self) -> bool {
        self.socket.is_bootstrap_connection()
    }

    pub fn is_realtime_connection(&self) -> bool {
        self.socket.is_realtime_connection()
    }

    pub fn queue_realtime(&self, message: Box<dyn Message>) {
        self.tcp_message_manager.put_message(TcpMessageItem {
            message: Some(message),
            endpoint: *self.remote_endpoint.lock().unwrap(),
            node_id: *self.remote_node_id.lock().unwrap(),
            socket: Some(Arc::clone(&self.socket)),
        });
    }
}

impl Drop for BootstrapServer {
    fn drop(&mut self) {
        let remote_ep = { self.remote_endpoint.lock().unwrap().clone() };
        self.observer.boostrap_server_exited(
            self.socket.socket_type(),
            self.unique_id(),
            remote_ep,
        );
        self.stop();
    }
}

pub trait RequestResponseVisitorFactory {
    fn handshake_visitor(&self, server: Arc<BootstrapServer>) -> Box<dyn HandshakeMessageVisitor>;

    fn realtime_visitor(&self, server: Arc<BootstrapServer>) -> Box<dyn RealtimeMessageVisitor>;

    fn bootstrap_visitor(&self, server: Arc<BootstrapServer>) -> Box<dyn BootstrapMessageVisitor>;

    fn handle(&self) -> *mut c_void;
}

pub trait HandshakeMessageVisitor: MessageVisitor {
    fn process(&self) -> bool;
    fn bootstrap(&self) -> bool;
    fn as_message_visitor(&self) -> &dyn MessageVisitor;
}

pub trait RealtimeMessageVisitor: MessageVisitor {
    fn process(&self) -> bool;
    fn as_message_visitor(&self) -> &dyn MessageVisitor;
}

pub trait BootstrapMessageVisitor: MessageVisitor {
    fn processed(&self) -> bool;
    fn as_message_visitor(&self) -> &dyn MessageVisitor;
}

pub trait BootstrapServerExt {
    fn start(&self);
    fn timeout(&self);

    fn receive_message(&self);
    fn received_message(&self, message: Option<Box<dyn Message>>);
    fn process_message(&self, message: Box<dyn Message>) -> bool;
}

impl BootstrapServerExt for Arc<BootstrapServer> {
    fn start(&self) {
        // Set remote_endpoint
        let mut guard = self.remote_endpoint.lock().unwrap();
        if guard.port() == 0 {
            if let Some(ep) = self.socket.get_remote() {
                *guard = ep;
            }
            debug_assert!(guard.port() != 0);
        }
        self.receive_message();
    }

    fn timeout(&self) {
        if self.socket.has_timed_out() {
            self.observer.bootstrap_server_timeout(self.unique_id());
            self.socket.close();
        }
    }

    fn receive_message(&self) {
        if self.is_stopped() {
            return;
        }

        let self_clone = Arc::clone(self);
        self.message_deserializer.read(
            Arc::clone(&self.socket),
            Box::new(move |ec, msg| {
                if ec.is_err() {
                    // IO error or critical error when deserializing message
                    let _ = self_clone.stats.inc(
                        StatType::Error,
                        DetailType::from(self_clone.message_deserializer.status()),
                        Direction::In,
                    );
                    self_clone.stop();
                    return;
                }
                self_clone.received_message(msg);
            }),
        );
    }

    fn received_message(&self, message: Option<Box<dyn Message>>) {
        let mut should_continue = true;
        match message {
            Some(message) => {
                should_continue = self.process_message(message);
            }
            None => {
                // Error while deserializing message
                debug_assert!(self.message_deserializer.status() != ParseStatus::Success);
                let _ = self.stats.inc(
                    StatType::Error,
                    DetailType::from(self.message_deserializer.status()),
                    Direction::In,
                );
                if self.message_deserializer.status() == ParseStatus::DuplicatePublishMessage {
                    let _ = self.stats.inc(
                        StatType::Filter,
                        DetailType::DuplicatePublish,
                        Direction::In,
                    );
                }
            }
        }

        if should_continue {
            self.receive_message();
        }
    }

    fn process_message(&self, message: Box<dyn Message>) -> bool {
        let _ = self.stats.inc(
            StatType::BootstrapServer,
            DetailType::from(message.header().message_type()),
            Direction::In,
        );

        debug_assert!(
            self.is_undefined_connection()
                || self.is_realtime_connection()
                || self.is_bootstrap_connection()
        );

        /*
         * Server initially starts in undefined state, where it waits for either a handshake or booststrap request message
         * If the server receives a handshake (and it is successfully validated) it will switch to a realtime mode.
         * In realtime mode messages are deserialized and queued to `tcp_message_manager` for further processing.
         * In realtime mode any bootstrap requests are ignored.
         *
         * If the server receives a bootstrap request before receiving a handshake, it will switch to a bootstrap mode.
         * In bootstrap mode once a valid bootstrap request message is received, the server will start a corresponding bootstrap server and pass control to that server.
         * Once that server finishes its task, control is passed back to this server to read and process any subsequent messages.
         * In bootstrap mode any realtime messages are ignored
         */
        if self.is_undefined_connection() {
            let handshake_visitor = self
                .request_response_visitor_factory
                .handshake_visitor(Arc::clone(self));
            message.visit(handshake_visitor.as_message_visitor());

            if handshake_visitor.process() {
                self.queue_realtime(message);
                return true;
            } else if handshake_visitor.bootstrap() {
                // Switch to bootstrap connection mode and handle message in subsequent bootstrap visitor
                self.to_bootstrap_connection();
            } else {
                // Neither handshake nor bootstrap received when in handshake mode
                return true;
            }
        } else if self.is_realtime_connection() {
            let realtime_visitor = self
                .request_response_visitor_factory
                .realtime_visitor(Arc::clone(self));
            message.visit(realtime_visitor.as_message_visitor());
            if realtime_visitor.process() {
                self.queue_realtime(message);
            }
            return true;
        }
        // It is possible for server to switch to bootstrap mode immediately after processing handshake, thus no `else if`
        if self.is_bootstrap_connection() {
            let bootstrap_visitor = self
                .request_response_visitor_factory
                .bootstrap_visitor(Arc::clone(self));
            message.visit(bootstrap_visitor.as_message_visitor());
            return !bootstrap_visitor.processed(); // Stop receiving new messages if bootstrap serving started
        }
        debug_assert!(false);
        true // Continue receiving new messages
    }
}
