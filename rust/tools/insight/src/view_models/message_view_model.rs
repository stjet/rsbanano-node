use crate::message_recorder::RecordedMessage;
use rsnano_network::ChannelDirection;

#[derive(Clone)]
pub(crate) struct MessageViewModel {
    pub channel_id: String,
    pub direction: String,
    pub message_type: String,
    pub message: String,
}

impl From<RecordedMessage> for MessageViewModel {
    fn from(value: RecordedMessage) -> Self {
        Self {
            channel_id: value.channel_id.to_string(),
            direction: if value.direction == ChannelDirection::Inbound {
                "in".into()
            } else {
                "out".into()
            },
            message_type: format!("{:?}", value.message.message_type()),
            message: format!("{:#?}", value.message),
        }
    }
}
