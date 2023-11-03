use crate::utils::{deserialize_block, BlockUniquer};
use anyhow::Result;
use rsnano_core::{
    serialized_block_size,
    utils::{Deserialize, FixedSizeSerialize, Stream},
    BlockEnum, BlockHash, BlockType, Root,
};
use std::{
    fmt::{Debug, Display, Write},
    sync::Arc,
};

use super::{MessageHeader, MessageType};

#[derive(Clone, PartialEq, Eq)]
pub struct ConfirmReq {
    header: MessageHeader,
    pub payload: ConfirmReqPayload,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConfirmReqPayload {
    pub block: Option<Arc<BlockEnum>>,
    pub roots_hashes: Vec<(BlockHash, Root)>,
}

impl ConfirmReqPayload {
    pub fn deserialize(
        stream: &mut impl Stream,
        header: &MessageHeader,
        uniquer: Option<&BlockUniquer>,
    ) -> Result<Self> {
        debug_assert!(header.message_type == MessageType::ConfirmReq);
        if header.block_type() == BlockType::NotABlock {
            Ok(Self {
                block: None,
                roots_hashes: Self::deserialize_roots(stream, &header)?,
            })
        } else {
            Ok(Self {
                block: Some(deserialize_block(header.block_type(), stream, uniquer)?),
                roots_hashes: Vec::new(),
            })
        }
    }

    fn deserialize_roots(
        stream: &mut impl Stream,
        header: &MessageHeader,
    ) -> Result<Vec<(BlockHash, Root)>> {
        let count = header.count() as usize;
        let mut roots_hashes = Vec::with_capacity(count);
        for _ in 0..count {
            let block_hash = BlockHash::deserialize(stream)?;
            let root = Root::deserialize(stream)?;
            if !block_hash.is_zero() || !root.is_zero() {
                roots_hashes.push((block_hash, root));
            }
        }

        if roots_hashes.is_empty() || roots_hashes.len() != count {
            bail!("roots hashes empty or incorrect count");
        }

        Ok(roots_hashes)
    }

    pub fn serialize(&self, stream: &mut dyn Stream) -> Result<()> {
        if let Some(block) = &self.block {
            block.serialize(stream)?;
        } else {
            // Write hashes & roots
            for (hash, root) in &self.roots_hashes {
                stream.write_bytes(hash.as_bytes())?;
                stream.write_bytes(root.as_bytes())?;
            }
        }
        Ok(())
    }

    pub fn roots_string(&self) -> String {
        let mut result = String::new();
        for (hash, root) in &self.roots_hashes {
            write!(&mut result, "{}:{}, ", hash, root).unwrap();
        }
        result
    }

    pub fn serialized_size(block_type: BlockType, count: u8) -> usize {
        let mut result = 0;
        if block_type != BlockType::Invalid && block_type != BlockType::NotABlock {
            result = serialized_block_size(block_type);
        } else if block_type == BlockType::NotABlock {
            result = count as usize * (BlockHash::serialized_size() + Root::serialized_size());
        }
        result
    }
}

impl Display for ConfirmReqPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(block) = &self.block {
            write!(f, "\n{}", block.to_json().map_err(|_| std::fmt::Error)?)?;
        } else {
            for (hash, root) in &self.roots_hashes {
                write!(f, "\n{}:{}", hash, root)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::messages::MessageEnum;

    use super::*;
    use rsnano_core::{utils::MemoryStream, StateBlockBuilder};

    #[test]
    fn serialize_block() -> Result<()> {
        let block = Arc::new(StateBlockBuilder::new().build());
        let confirm_req1 = MessageEnum::new_confirm_req_with_block(&Default::default(), block);
        let confirm_req2 = serialize_and_deserialize(&confirm_req1)?;
        assert_eq!(confirm_req1, confirm_req2);
        Ok(())
    }

    #[test]
    fn serialze_roots_hashes() -> Result<()> {
        let roots_hashes = vec![(BlockHash::from(1), Root::from(2))];
        let confirm_req1 =
            MessageEnum::new_confirm_req_with_roots_hashes(&Default::default(), roots_hashes);
        let confirm_req2 = serialize_and_deserialize(&confirm_req1)?;
        assert_eq!(confirm_req1, confirm_req2);
        Ok(())
    }

    #[test]
    fn serialze_many_roots_hashes() -> Result<()> {
        let roots_hashes = (0..7)
            .into_iter()
            .map(|i| (BlockHash::from(i), Root::from(i + 1)))
            .collect();
        let confirm_req1 =
            MessageEnum::new_confirm_req_with_roots_hashes(&Default::default(), roots_hashes);
        let confirm_req2 = serialize_and_deserialize(&confirm_req1)?;
        assert_eq!(confirm_req1, confirm_req2);
        Ok(())
    }

    fn serialize_and_deserialize(confirm_req1: &MessageEnum) -> Result<MessageEnum, anyhow::Error> {
        let mut stream = MemoryStream::new();
        confirm_req1.serialize(&mut stream)?;
        let header = MessageHeader::deserialize(&mut stream)?;
        MessageEnum::deserialize(&mut stream, header, 0, None, None)
    }
}
