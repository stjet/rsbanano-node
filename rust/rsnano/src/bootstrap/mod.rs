use crate::{
    encode_hex,
    logger_mt::Logger,
    websocket::{Listener, MessageBuilder},
    HardenedConstants,
};
use anyhow::Result;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(FromPrimitive)]
pub(crate) enum BootstrapMode {
    Legacy,
    Lazy,
    WalletLazy,
}

pub(crate) struct BootstrapAttempt {
    pub id: String,
    pub mode: BootstrapMode,
    pub total_blocks: AtomicU64,
    next_log: Mutex<Instant>,
    logger: Arc<dyn Logger>,
    websocket_server: Arc<dyn Listener>,
    attempt_start: Instant,
}

impl BootstrapAttempt {
    pub(crate) fn new(
        logger: Arc<dyn Logger>,
        websocket_server: Arc<dyn Listener>,
        id: &str,
        mode: BootstrapMode,
    ) -> Result<Self> {
        let id = if id.is_empty() {
            encode_hex(HardenedConstants::get().random_128)
        } else {
            id.to_owned()
        };

        let result = Self {
            id,
            next_log: Mutex::new(Instant::now()),
            logger,
            mode,
            websocket_server,
            attempt_start: Instant::now(),
            total_blocks: AtomicU64::new(0),
        };

        result.start()?;
        Ok(result)
    }

    fn start(&self) -> Result<()> {
        let mode = self.mode_text();
        let id = &self.id;
        self.logger
            .always_log(&format!("Starting {mode} bootstrap attempt with ID {id}"));
        self.websocket_server
            .broadcast(&MessageBuilder::bootstrap_started(id, mode)?)?;
        Ok(())
    }

    pub(crate) fn should_log(&self) -> bool {
        let mut next_log = self.next_log.lock().unwrap();
        let now = Instant::now();
        if *next_log < now {
            *next_log = now + Duration::from_secs(15);
            true
        } else {
            false
        }
    }

    pub(crate) fn mode_text(&self) -> &'static str {
        match self.mode {
            BootstrapMode::Legacy => "legacy",
            BootstrapMode::Lazy => "lazy",
            BootstrapMode::WalletLazy => "wallet_lazy",
        }
    }
}

impl Drop for BootstrapAttempt {
    fn drop(&mut self) {
        let mode = self.mode_text();
        let id = &self.id;
        self.logger
            .always_log(&format!("Exiting {mode} bootstrap attempt with ID {id}"));

        let duration = self.attempt_start.elapsed();
        self.websocket_server
            .broadcast(
                &MessageBuilder::bootstrap_exited(
                    id,
                    mode,
                    duration,
                    self.total_blocks.load(Ordering::SeqCst),
                )
                .unwrap(),
            )
            .unwrap();
    }
}
