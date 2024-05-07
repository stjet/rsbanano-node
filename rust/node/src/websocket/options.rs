use rsnano_core::utils::PropertyTree;

use super::{ConfirmationOptions, Message, VoteOptions};

#[derive(Clone)]
pub enum Options {
    Confirmation(ConfirmationOptions),
    Vote(VoteOptions),
    Other,
}

impl Options {
    /**
     * Checks if a message should be filtered for default options (no options given).
     * @param message_a the message to be checked
     * @return false - the message should always be broadcasted
     */
    pub fn should_filter(&self, message: &Message) -> bool {
        match self {
            Options::Confirmation(i) => i.should_filter(message),
            Options::Vote(i) => i.should_filter(message),
            Options::Other => false,
        }
    }

    /**
     * Update options, if available for a given topic
     * @return false on success
     */
    pub fn update(&mut self, options: &dyn PropertyTree) {
        if let Options::Confirmation(i) = self {
            i.update(options);
        }
    }
}
