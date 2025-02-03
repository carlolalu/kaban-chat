use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;
use tokio::sync;

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

/// Dispatch is the type of packet unit used exclusively by the server. It associates a message
/// with an identifier, so that the clients can avoid to receive their own messages in cases of
/// multiple equal nicknames.
#[derive(Debug, Clone)]
pub struct Dispatch {
    userid: usize,
    msg: Message,
}

impl Dispatch {
    pub fn new(userid: usize, msg: Message) -> Dispatch {
        Dispatch { userid, msg }
    }

    pub fn into_msg(self) -> Message {
        self.msg
    }

    pub fn get_userid(&self) -> usize {
        self.userid
    }
}

/// This function reads messages from a Reader and forward them through its channel.
pub async fn handle_msgs_reader(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    tx: sync::mpsc::Sender<Message>,
) {
    let buffer_incoming: Vec<u8> = Vec::with_capacity(1000);
}
