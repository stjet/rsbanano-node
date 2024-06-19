use rsnano_core::KeyPair;
use rsnano_ledger::Ledger;

use super::{
    InboundMessageQueue, Network, NullSocketObserver, OutboundBandwidthLimiter, ResponseServerImpl,
    Socket, SynCookies,
};
use crate::{
    block_processing::BlockProcessor,
    bootstrap::{BootstrapConnectionsConfig, BootstrapInitiator, BootstrapMessageVisitorFactory},
    config::{NodeConfig, NodeFlags},
    stats::Stats,
    utils::{AsyncRuntime, ThreadPool, ThreadPoolImpl},
    NetworkParams,
};
use std::sync::Arc;

pub(crate) struct ResponseServerFactory {
    pub(crate) runtime: Arc<AsyncRuntime>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) node_id: KeyPair,
    pub(crate) ledger: Arc<Ledger>,
    pub(crate) workers: Arc<dyn ThreadPool>,
    pub(crate) block_processor: Arc<BlockProcessor>,
    pub(crate) bootstrap_initiator: Arc<BootstrapInitiator>,
    pub(crate) network: Arc<Network>,
    pub(crate) inbound_queue: Arc<InboundMessageQueue>,
    pub(crate) node_flags: NodeFlags,
    pub(crate) network_params: NetworkParams,
    pub(crate) node_config: NodeConfig,
    pub(crate) syn_cookies: Arc<SynCookies>,
}

impl ResponseServerFactory {
    pub(crate) fn new_null() -> Self {
        let ledger = Arc::new(Ledger::new_null());
        let node_config = NodeConfig::new_test_instance();
        let flags = NodeFlags::default();
        let network = Arc::new(Network::new_null());
        let runtime = Arc::new(AsyncRuntime::default());
        let workers = Arc::new(ThreadPoolImpl::new_test_instance());
        let network_params = NetworkParams::new(rsnano_core::Networks::NanoDevNetwork);
        let stats = Arc::new(Stats::default());
        let block_processor = Arc::new(BlockProcessor::new_test_instance(ledger.clone()));
        Self {
            runtime: runtime.clone(),
            stats: stats.clone(),
            node_id: KeyPair::from(42),
            ledger: ledger.clone(),
            workers: Arc::new(ThreadPoolImpl::new_test_instance()),
            block_processor: block_processor.clone(),
            bootstrap_initiator: Arc::new(BootstrapInitiator::new(
                node_config.clone(),
                BootstrapConnectionsConfig::default(),
                flags.clone(),
                network.clone(),
                runtime,
                workers,
                network_params.clone(),
                Arc::new(NullSocketObserver::new()),
                stats,
                Arc::new(OutboundBandwidthLimiter::default()),
                block_processor,
                None,
                ledger,
            )),
            network,
            inbound_queue: Arc::new(InboundMessageQueue::default()),
            node_flags: flags,
            network_params,
            node_config,
            syn_cookies: Arc::new(SynCookies::new(1)),
        }
    }

    pub(crate) fn create_response_server(&self, socket: Arc<Socket>) -> Arc<ResponseServerImpl> {
        let message_visitor_factory = Arc::new(BootstrapMessageVisitorFactory::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.stats),
            self.network_params.network.clone(),
            self.node_id.clone(),
            Arc::clone(&self.ledger),
            Arc::clone(&self.workers),
            Arc::clone(&self.block_processor),
            Arc::clone(&self.bootstrap_initiator),
            self.node_flags.clone(),
        ));

        Arc::new(ResponseServerImpl::new(
            &self.network.clone(),
            self.inbound_queue.clone(),
            socket,
            Arc::new(self.node_config.clone()),
            Arc::clone(&self.network.publish_filter),
            Arc::new(self.network_params.clone()),
            Arc::clone(&self.stats),
            message_visitor_factory,
            true,
            self.syn_cookies.clone(),
            self.node_id.clone(),
        ))
    }
}
