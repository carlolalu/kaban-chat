// ########################################
use serde::{Deserialize, Serialize};

// ########################################
pub mod constant {
    pub const SERVER_ADDR: &str = "127.0.0.1:6440";
    pub const MAX_NUM_USERS: usize = 2000;
}

// ########################################
/// Message is the type of packet unit that the client uses. They comprehend a username of at most
/// MAX_USERNAME_LEN chars and a content of at most MAX_CONTENT_LEN chars.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    username: String,
    content: String,
}

impl Message {
    /// Maximal length (in chars) of the username.
    pub const MAX_USERNAME_LEN: usize = 32;

    /// Maximal length (in chars) of the content of the message.
    pub const MAX_CONTENT_LEN: usize = 256;

    pub fn new(username: &str, content: &str) -> Message {
        if username.len() > Message::MAX_USERNAME_LEN {
            panic!("username too long");
        }

        if content.len() > Message::MAX_CONTENT_LEN {
            panic!("content too long");
        }

        Message {
            username: username.to_string(),
            content: content.to_string(),
        }
    }

    pub fn get_username(&self) -> String {
        self.username.clone()
    }

    pub fn get_content(&self) -> String {
        self.content.clone()
    }
}
