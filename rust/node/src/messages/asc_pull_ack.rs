use num_traits::FromPrimitive;
use rsnano_core::{
    deserialize_block_enum, serialize_block_enum,
    utils::{Deserialize, MemoryStream, Serialize, Stream, StreamExt},
    Account, BlockEnum, BlockHash, BlockType,
};
use std::{any::Any, fmt::Display, mem::size_of};

use super::{AscPullPayloadId, Message, MessageHeader, MessageType, MessageVisitor, ProtocolInfo};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AscPullAckType {
    Blocks(BlocksAckPayload),
    AccountInfo(AccountInfoAckPayload),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AscPullAckPayload {
    pub id: u64,
    pub pull_type: AscPullAckType,
}

impl AscPullAckPayload {
    pub fn deserialize(stream: &mut impl Stream, header: &MessageHeader) -> anyhow::Result<Self> {
        debug_assert!(header.message_type == MessageType::AscPullAck);
        let pull_type_code = AscPullPayloadId::from_u8(stream.read_u8()?)
            .ok_or_else(|| anyhow!("Unknown asc_pull_type"))?;
        let id = stream.read_u64_be()?;
        let pull_type = match pull_type_code {
            AscPullPayloadId::Invalid => bail!("Unknown asc_pull_type"),
            AscPullPayloadId::Blocks => {
                let mut payload = BlocksAckPayload::default();
                payload.deserialize(stream)?;
                AscPullAckType::Blocks(payload)
            }
            AscPullPayloadId::AccountInfo => {
                let mut payload = AccountInfoAckPayload::default();
                payload.deserialize(stream)?;
                AscPullAckType::AccountInfo(payload)
            }
        };

        Ok(AscPullAckPayload { id, pull_type })
    }

    pub fn payload_type(&self) -> AscPullPayloadId {
        match self.pull_type {
            AscPullAckType::Blocks(_) => AscPullPayloadId::Blocks,
            AscPullAckType::AccountInfo(_) => AscPullPayloadId::AccountInfo,
        }
    }

    fn serialize_pull_type(&self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        match &self.pull_type {
            AscPullAckType::Blocks(blocks) => blocks.serialize(stream),
            AscPullAckType::AccountInfo(account_info) => account_info.serialize(stream),
        }
    }

    fn serialize(&self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        stream.write_u8(self.payload_type() as u8)?;
        stream.write_u64_be(self.id)?;
        self.serialize_pull_type(stream)
    }

    pub fn serialized_size(header: &MessageHeader) -> usize {
        let payload_length = header.extensions.data as usize;

        size_of::<u8>() // type code 
        + size_of::<u64>() // id
        + payload_length
    }
}

impl Display for AscPullAckPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.pull_type {
            AscPullAckType::Blocks(blocks) => {
                for block in &blocks.blocks {
                    write!(f, "{}", block.to_json().map_err(|_| std::fmt::Error)?)?;
                }
            }
            AscPullAckType::AccountInfo(info) => {
                write!(
                    f,
                    "account public key:{} account open:{} account head:{} block count:{} confirmation frontier:{} confirmation height:{}",
                    info.account.encode_account(),
                    info.account_open,
                    info.account_head,
                    info.account_block_count,
                    info.account_conf_frontier,
                    info.account_conf_height,
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct BlocksAckPayload {
    pub blocks: Vec<BlockEnum>,
}

impl BlocksAckPayload {
    /* Header allows for 16 bit extensions; 65535 bytes / 500 bytes (block size with some future margin) ~ 131 */
    pub const MAX_BLOCKS: usize = 128;

    pub fn deserialize(&mut self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        while let Ok(current) = deserialize_block_enum(stream) {
            if self.blocks.len() >= Self::MAX_BLOCKS {
                bail!("too many blocks")
            }
            self.blocks.push(current);
        }
        Ok(())
    }

    pub fn serialize(&self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        if self.blocks.len() > Self::MAX_BLOCKS {
            bail!("too many blocks");
        }

        for block in &self.blocks {
            serialize_block_enum(stream, block)?;
        }
        // For convenience, end with null block terminator
        stream.write_u8(BlockType::NotABlock as u8)
    }
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct AccountInfoAckPayload {
    pub account: Account,
    pub account_open: BlockHash,
    pub account_head: BlockHash,
    pub account_block_count: u64,
    pub account_conf_frontier: BlockHash,
    pub account_conf_height: u64,
}

impl AccountInfoAckPayload {
    pub fn serialize(&self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        self.account.serialize(stream)?;
        self.account_open.serialize(stream)?;
        self.account_head.serialize(stream)?;
        stream.write_u64_be(self.account_block_count)?;
        self.account_conf_frontier.serialize(stream)?;
        stream.write_u64_be(self.account_conf_height)
    }

    pub fn deserialize(&mut self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        self.account = Account::deserialize(stream)?;
        self.account_open = BlockHash::deserialize(stream)?;
        self.account_head = BlockHash::deserialize(stream)?;
        self.account_block_count = stream.read_u64_be()?;
        self.account_conf_frontier = BlockHash::deserialize(stream)?;
        self.account_conf_height = stream.read_u64_be()?;
        Ok(())
    }

    pub(crate) fn test_data() -> AccountInfoAckPayload {
        Self {
            account: Account::from(1),
            account_open: BlockHash::from(2),
            account_head: BlockHash::from(3),
            account_block_count: 4,
            account_conf_frontier: BlockHash::from(5),
            account_conf_height: 3,
        }
    }
}

#[derive(Clone)]
pub struct AscPullAck {
    pub header: MessageHeader,
    pub payload: AscPullAckPayload,
}

impl AscPullAck {
    pub fn ack_blocks(protocol_info: &ProtocolInfo, id: u64, blocks: Vec<BlockEnum>) -> Self {
        let mut header = MessageHeader::new(MessageType::AscPullAck, protocol_info);
        let mut stream = MemoryStream::new();
        let blocks = BlocksAckPayload { blocks };
        blocks.serialize(&mut stream).unwrap(); // can't fail
        let payload_len: u16 = stream.bytes_written() as u16;
        header.extensions.data = payload_len;
        Self {
            header,
            payload: AscPullAckPayload {
                id,
                pull_type: AscPullAckType::Blocks(blocks),
            },
        }
    }

    pub fn ack_accounts(
        protocol_info: &ProtocolInfo,
        id: u64,
        accounts: AccountInfoAckPayload,
    ) -> Self {
        let mut header = MessageHeader::new(MessageType::AscPullAck, protocol_info);
        let mut stream = MemoryStream::new();
        accounts.serialize(&mut stream).unwrap(); // can't fail
        let payload_len: u16 = stream.bytes_written() as u16;
        header.extensions.data = payload_len;
        Self {
            header,
            payload: AscPullAckPayload {
                id,
                pull_type: AscPullAckType::AccountInfo(accounts),
            },
        }
    }

    pub fn deserialize_asc_pull_ack(
        stream: &mut impl Stream,
        header: MessageHeader,
    ) -> anyhow::Result<Self> {
        let payload = AscPullAckPayload::deserialize(stream, &header)?;
        Ok(Self { header, payload })
    }
}

impl Message for AscPullAck {
    fn header(&self) -> &MessageHeader {
        &self.header
    }

    fn set_header(&mut self, header: &MessageHeader) {
        self.header = header.clone();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn serialize(&self, stream: &mut dyn Stream) -> anyhow::Result<()> {
        self.header.serialize(stream)?;
        self.payload.serialize(stream)
    }

    fn visit(&self, visitor: &mut dyn MessageVisitor) {
        visitor.asc_pull_ack(self);
    }

    fn clone_box(&self) -> Box<dyn Message> {
        Box::new(self.clone())
    }

    fn message_type(&self) -> MessageType {
        MessageType::AscPullAck
    }
}

impl Display for AscPullAck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.header)?;
        self.payload.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_core::{utils::MemoryStream, BlockBuilder};

    #[test]
    fn serialize_header() -> anyhow::Result<()> {
        let original = AscPullAck::ack_blocks(&ProtocolInfo::dev_network(), 0, vec![]);

        let mut stream = MemoryStream::new();
        original.serialize(&mut stream)?;

        let header = MessageHeader::from_stream(&mut stream)?;
        assert_eq!(header.message_type, MessageType::AscPullAck);
        Ok(())
    }

    #[test]
    fn serialize_blocks() -> anyhow::Result<()> {
        let original = AscPullAck::ack_blocks(
            &ProtocolInfo::dev_network(),
            7,
            vec![BlockBuilder::state().build(), BlockBuilder::state().build()],
        );

        let mut stream = MemoryStream::new();
        original.serialize(&mut stream)?;

        let header = MessageHeader::from_stream(&mut stream)?;
        let message_out = AscPullAck::deserialize_asc_pull_ack(&mut stream, header)?;
        assert_eq!(message_out.payload, original.payload);
        assert!(stream.at_end());
        Ok(())
    }

    #[test]
    fn serialize_account_info() -> anyhow::Result<()> {
        let original = AscPullAck::ack_accounts(
            &ProtocolInfo::dev_network(),
            7,
            AccountInfoAckPayload {
                account: Account::from(1),
                account_open: BlockHash::from(2),
                account_head: BlockHash::from(3),
                account_block_count: 4,
                account_conf_frontier: BlockHash::from(5),
                account_conf_height: 6,
            },
        );

        let mut stream = MemoryStream::new();
        original.serialize(&mut stream)?;

        let header = MessageHeader::from_stream(&mut stream)?;
        let message_out = AscPullAck::deserialize_asc_pull_ack(&mut stream, header)?;
        assert_eq!(message_out.payload, original.payload);
        assert!(stream.at_end());
        Ok(())
    }

    #[test]
    fn display() {
        let ack = AscPullAck::ack_accounts(
            &ProtocolInfo::dev_network(),
            7,
            AccountInfoAckPayload {
                account: Account::from(1),
                account_open: BlockHash::from(2),
                account_head: BlockHash::from(3),
                account_block_count: 4,
                account_conf_frontier: BlockHash::from(5),
                account_conf_height: 6,
            },
        );

        assert_eq!(ack.to_string(), "NetID: 5241(dev), VerMaxUsingMin: 19/19/18, MsgType: 15(asc_pull_ack), Extensions: 0090\naccount public key:nano_1111111111111111111111111111111111111111111111111113b8661hfk account open:0000000000000000000000000000000000000000000000000000000000000002 account head:0000000000000000000000000000000000000000000000000000000000000003 block count:4 confirmation frontier:0000000000000000000000000000000000000000000000000000000000000005 confirmation height:6");
    }
}
