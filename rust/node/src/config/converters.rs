use super::{AccountSetsToml, GlobalConfig};
use crate::{
    block_processing::BlockProcessorConfig,
    bootstrap::{AccountSetsConfig, BootstrapAscendingConfig, BootstrapConnectionsConfig},
};
use std::time::Duration;

impl From<&GlobalConfig> for BlockProcessorConfig {
    fn from(value: &GlobalConfig) -> Self {
        let config = &value.node_config.block_processor;
        Self {
            max_peer_queue: config.max_peer_queue,
            priority_local: config.priority_local,
            priority_bootstrap: config.priority_bootstrap,
            priority_live: config.priority_live,
            max_system_queue: config.max_system_queue,
            batch_max_time: Duration::from_millis(
                value.node_config.block_processor_batch_max_time_ms as u64,
            ),
            full_size: value.flags.block_processor_full_size,
            batch_size: value.flags.block_processor_batch_size,
            work_thresholds: value.network_params.work.clone(),
        }
    }
}

impl From<&GlobalConfig> for BootstrapAscendingConfig {
    fn from(value: &GlobalConfig) -> Self {
        let config = &value.node_config.bootstrap_ascending;
        Self {
            requests_limit: config.requests_limit,
            database_requests_limit: config.database_requests_limit,
            pull_count: config.pull_count,
            timeout: config.timeout,
            throttle_coefficient: config.throttle_coefficient,
            throttle_wait: config.throttle_wait,
            account_sets: (&config.account_sets).into(),
            block_wait_count: config.block_wait_count,
            min_protocol_version: value.network_params.network.bootstrap_protocol_version_min,
        }
    }
}

impl From<&AccountSetsToml> for AccountSetsConfig {
    fn from(value: &AccountSetsToml) -> Self {
        Self {
            consideration_count: value.consideration_count,
            priorities_max: value.priorities_max,
            blocking_max: value.blocking_max,
            cooldown: value.cooldown,
        }
    }
}

impl From<&AccountSetsConfig> for AccountSetsToml {
    fn from(value: &AccountSetsConfig) -> Self {
        Self {
            consideration_count: value.consideration_count,
            priorities_max: value.priorities_max,
            blocking_max: value.blocking_max,
            cooldown: value.cooldown,
        }
    }
}

impl From<&GlobalConfig> for BootstrapConnectionsConfig {
    fn from(value: &GlobalConfig) -> Self {
        Self {
            bootstrap_connections: value.node_config.bootstrap_connections,
            bootstrap_connections_max: value.node_config.bootstrap_connections_max,
            tcp_io_timeout: Duration::from_secs(value.node_config.tcp_io_timeout_s as u64),
            silent_connection_tolerance_time: Duration::from_secs(
                value
                    .network_params
                    .network
                    .silent_connection_tolerance_time_s as u64,
            ),
            allow_bootstrap_peers_duplicates: value.flags.allow_bootstrap_peers_duplicates,
            disable_legacy_bootstrap: value.flags.disable_legacy_bootstrap,
            idle_timeout: value.network_params.network.idle_timeout,
            lazy_max_pull_blocks: value.network_params.bootstrap.lazy_max_pull_blocks,
            work_thresholds: value.network_params.work.clone(),
            lazy_retry_limit: value.network_params.bootstrap.lazy_retry_limit,
            protocol: value.network_params.network.protocol_info(),
        }
    }
}
