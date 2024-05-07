use super::{
    to_topic, ConfirmationJsonOptions, ConfirmationOptions, Message, Options, Topic,
    VoteJsonOptions, VoteOptions,
};
use crate::{wallets::Wallets, websocket::IncomingMessage};
use futures_util::{SinkExt, StreamExt};
use rsnano_core::utils::{milliseconds_since_epoch, PropertyTree, SerdePropertyTree};
use serde_json::Value;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, trace, warn};

pub struct WebsocketSessionEntry {
    /// Map of subscriptions -> options registered by this session.
    pub subscriptions: Mutex<HashMap<Topic, Options>>,
    send_queue_tx: mpsc::Sender<Message>,
    tx_close: Mutex<Option<oneshot::Sender<()>>>,
}

impl WebsocketSessionEntry {
    pub fn new(send_queue_tx: mpsc::Sender<Message>, tx_close: oneshot::Sender<()>) -> Self {
        Self {
            subscriptions: Mutex::new(HashMap::new()),
            send_queue_tx,
            tx_close: Mutex::new(Some(tx_close)),
        }
    }

    pub fn blocking_write(&self, msg: Message) -> anyhow::Result<()> {
        if !self.should_filter(&msg) {
            self.send_queue_tx.blocking_send(msg)?;
        }
        Ok(())
    }

    pub async fn write(&self, msg: Message) -> anyhow::Result<()> {
        if !self.should_filter(&msg) {
            self.send_queue_tx.send(msg).await?
        }
        Ok(())
    }

    pub fn close(&self) {
        if let Some(close) = self.tx_close.lock().unwrap().take() {
            let _ = close.send(());
        }
    }

    fn should_filter(&self, msg: &Message) -> bool {
        if msg.topic == Topic::Ack {
            return false;
        }

        let subs = self.subscriptions.lock().unwrap();
        if let Some(options) = subs.get(&msg.topic) {
            options.should_filter(&msg)
        } else {
            true
        }
    }
}

pub struct WebsocketSession {
    entry: Arc<WebsocketSessionEntry>,
    wallets: Arc<Wallets>,
    topic_subscriber_count: Arc<[AtomicUsize; 11]>,
    remote_endpoint: SocketAddr,
}

impl WebsocketSession {
    pub fn new(
        wallets: Arc<Wallets>,
        topic_subscriber_count: Arc<[AtomicUsize; 11]>,
        remote_endpoint: SocketAddr,
        entry: Arc<WebsocketSessionEntry>,
    ) -> Self {
        trace!(remote = %remote_endpoint, "new websocket session created");
        Self {
            entry,
            wallets,
            topic_subscriber_count,
            remote_endpoint,
        }
    }

    pub async fn run(
        self,
        stream: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        send_queue: &mut mpsc::Receiver<Message>,
    ) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(msg) = stream.next() =>{
                    if !self.process(msg?).await {
                        break;
                    }
                }
                Some(msg) = send_queue.recv() =>{
                    let message_text = msg.contents.to_json();
                    trace!(message = message_text, "sending websocket message");
                    // write queued messages
                    stream
                        .send(tokio_tungstenite::tungstenite::Message::text(
                            message_text,
                        )).await?;
                }
                else =>{
                    break;
                }
            }
        }
        Ok(())
    }

    async fn process(&self, msg: tokio_tungstenite::tungstenite::Message) -> bool {
        if msg.is_close() {
            trace!("close message received");
            false
        } else if msg.is_text() {
            let msg_text = match msg.into_text() {
                Ok(i) => i,
                Err(e) => {
                    warn!("Could not deserialize string: {:?}", e);
                    return false;
                }
            };

            trace!(message = msg_text, "Received text websocket message");

            let incoming = match serde_json::from_str::<IncomingMessage>(&msg_text) {
                Ok(i) => i,
                Err(e) => {
                    warn!(
                        text = msg_text,
                        "Could not deserialize JSON message: {:?}", e
                    );
                    return false;
                }
            };

            if let Err(e) = self.handle_message(incoming).await {
                warn!("Could not process websocket message: {:?}", e);
                return false;
            }
            true
        } else {
            true
        }
    }

    async fn handle_message(&self, message: IncomingMessage<'_>) -> anyhow::Result<()> {
        let topic = to_topic(message.topic.unwrap_or(""));
        let mut action_succeeded = false;
        let mut ack = message.ack;
        let mut reply_action = message.action.unwrap_or("");
        if message.action == Some("subscribe") && topic != Topic::Invalid {
            let mut subs = self.entry.subscriptions.lock().unwrap();
            let options = match topic {
                Topic::Confirmation => {
                    if let Some(options_value) = message.options {
                        Options::Confirmation(ConfirmationOptions::new(
                            Arc::clone(&self.wallets),
                            serde_json::from_value::<ConfirmationJsonOptions>(options_value)?,
                        ))
                    } else {
                        Options::Other
                    }
                }
                Topic::Vote => {
                    if let Some(options_value) = message.options {
                        Options::Vote(VoteOptions::new(serde_json::from_value::<VoteJsonOptions>(
                            options_value,
                        )?))
                    } else {
                        Options::Other
                    }
                }
                _ => Options::Other,
            };
            let inserted = subs.insert(topic, options).is_none();
            if inserted {
                self.topic_subscriber_count[topic as usize].fetch_add(1, Ordering::SeqCst);
            }
            action_succeeded = true;
        } else if message.action == Some("update") {
            let mut subs = self.entry.subscriptions.lock().unwrap();
            if let Some(option) = subs.get_mut(&topic) {
                if let Some(options_value) = message.options {
                    option.update(&SerdePropertyTree::from_value(options_value));
                    action_succeeded = true;
                }
            }
        } else if message.action == Some("unsubscribe") && topic != Topic::Invalid {
            let mut subs = self.entry.subscriptions.lock().unwrap();
            if subs.remove(&topic).is_some() {
                info!(
                    "Removed subscription to topic: {} ({})",
                    topic.as_str(),
                    self.remote_endpoint
                );
                self.topic_subscriber_count[topic as usize].fetch_sub(1, Ordering::SeqCst);
            }
            action_succeeded = true;
        } else if message.action == Some("ping") {
            action_succeeded = true;
            ack = true;
            reply_action = "pong";
        }
        if ack && action_succeeded {
            self.send_ack(reply_action, &message.id).await?;
        }
        Ok(())
    }

    async fn send_ack(&self, reply_action: &str, id: &Option<&str>) -> anyhow::Result<()> {
        let mut vals = serde_json::Map::new();
        vals.insert("ack".to_string(), Value::String(reply_action.to_string()));
        vals.insert(
            "time".to_string(),
            Value::String(milliseconds_since_epoch().to_string()),
        );
        if let Some(id) = id {
            vals.insert("id".to_string(), Value::String(id.to_string()));
        }
        let contents = serde_json::Value::Object(vals);
        let msg = Message {
            topic: Topic::Ack,
            contents: SerdePropertyTree::from_value(contents),
        };

        self.entry.write(msg).await
    }
}

impl Drop for WebsocketSession {
    fn drop(&mut self) {
        trace!(remote = %self.remote_endpoint, "websocket session dropped");
        let subs = self.entry.subscriptions.lock().unwrap();
        for (topic, _) in subs.iter() {
            self.topic_subscriber_count[*topic as usize].fetch_sub(1, Ordering::SeqCst);
        }
    }
}
