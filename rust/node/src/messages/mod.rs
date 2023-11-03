use std::any::Any;

mod message_enum;
pub use message_enum::*;

mod message_header;
pub use message_header::*;

mod node_id_handshake;
pub use node_id_handshake::*;

mod keepalive;
pub use keepalive::*;

mod publish;
pub use publish::*;

mod confirm_req;
pub use confirm_req::*;

mod confirm_ack;
pub use confirm_ack::*;

mod frontier_req;
pub use frontier_req::*;

mod bulk_pull;
pub use bulk_pull::*;

mod bulk_pull_account;
pub use bulk_pull_account::*;

use rsnano_core::utils::{MemoryStream, Stream};

mod telemetry_ack;
pub use telemetry_ack::*;

mod asc_pull_req;
pub use asc_pull_req::*;

mod asc_pull_ack;
pub use asc_pull_ack::*;

use anyhow::Result;

pub trait Message: Send {
    fn header(&self) -> &MessageHeader;
    fn set_header(&mut self, header: &MessageHeader);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn serialize(&self, stream: &mut dyn Stream) -> Result<()>;
    fn visit(&self, visitor: &mut dyn MessageVisitor);
    fn clone_box(&self) -> Box<dyn Message>;
    fn message_type(&self) -> MessageType;
    fn to_bytes(&self) -> Vec<u8> {
        let mut stream = MemoryStream::new();
        self.serialize(&mut stream).unwrap();
        stream.to_vec()
    }
}

pub trait MessageVisitor {
    fn received(&mut self, message: &MessageEnum) {}
}

pub trait MessageExt {
    fn to_bytes(&self) -> Vec<u8>;
}
