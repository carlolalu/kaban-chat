use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync,
};

use tokio_stream::StreamExt;

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
    /// The delimiters to send serialised Messages in TCP connections. They are required to be
    /// different from '{', '}', and to be different from each others.
    pub(crate) const TCP_INIT_DELIMITER: u8 = b'|';
    pub(crate) const TCP_END_DELIMITER: u8 = b'`';

    /// Maximal length (in chars) of the username.
    pub const MAX_USERNAME_LEN: usize = 32;

    /// Maximal length (in chars) of the content of the message.
    pub const MAX_CONTENT_LEN: usize = 256;

    pub fn has_invalid_chars(text: &str) -> bool {
        assert_ne!(Message::TCP_INIT_DELIMITER, Message::TCP_END_DELIMITER);
        assert_ne!(Message::TCP_INIT_DELIMITER, b'{');
        assert_ne!(Message::TCP_INIT_DELIMITER, b'}');
        assert_ne!(Message::TCP_END_DELIMITER, b'{');
        assert_ne!(Message::TCP_END_DELIMITER, b'}');

        let init_delimiter = char::from(Message::TCP_INIT_DELIMITER);
        let end_delimiter = char::from(Message::TCP_END_DELIMITER);

        if text.contains(init_delimiter) || text.contains(end_delimiter) {
            true
        } else {
            false
        }
    }

    pub fn new(username: &str, content: &str) -> Message {
        if username.len() > Message::MAX_USERNAME_LEN || Self::has_invalid_chars(username) {
            panic!("username not valid: too long or invalid chars");
        }

        if content.len() > Message::MAX_CONTENT_LEN || Self::has_invalid_chars(content) {
            panic!("content not valid: too long or invalid chars");
        }

        Message {
            username: username.to_string(),
            content: content.to_string(),
        }
    }

    pub fn many_new(username: &str, text: &str) -> Vec<Message> {
        let msgs: Vec<_> = text
            .chars()
            .collect::<Vec<_>>()
            .chunks(Message::MAX_CONTENT_LEN)
            .map(|chars_content| chars_content.iter().collect::<String>())
            .map(|content| Message::new(username, &content))
            .collect();
        msgs
    }

    pub fn get_username(&self) -> String {
        self.username.clone()
    }

    pub fn get_content(&self) -> String {
        self.content.clone()
    }

    pub fn paket(self) -> String {
        let serialized = serde_json::to_string(&self).unwrap();
        let paketed = format!(
            "{}{serialized}{}",
            Message::TCP_INIT_DELIMITER,
            Message::TCP_END_DELIMITER
        );
        paketed
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
    // todo: cancellation token
) {
    let init_delimiter = Message::TCP_INIT_DELIMITER;
    let end_delimiter = Message::TCP_END_DELIMITER;

    // todo: avoid magic nums
    const BUFFER_LEN: usize = 1000;
    let mut buffer: Vec<u8> = Vec::with_capacity(BUFFER_LEN);
    let mut previous_fragment: Vec<u8> = Vec::with_capacity(BUFFER_LEN);

    // divide the chunks
    'process_reader: loop {
        buffer.clear();
        match reader.read_buf(&mut buffer).await {
            Err(_e) => panic!("Error by the reader"),
            Ok(n) if n > 0 => {
                // assumptions:
                // 1. previous fragment is initialised to smt, buffer as well
                // 2. previous_fragment does not have any delimiter

                let previous_fragment = &mut previous_fragment;
                let actual_fragments: Vec<&[u8]> =
                    buffer.split(|&byte| byte == end_delimiter).collect();

                let (first_frag, actual_fragments) = match actual_fragments.split_first() {
                    Some(smt) => (*smt.0, smt.1),
                    None => panic!("The split iterator of the buffer is empty even though the reader read a non-null amount of bytes!"),
                };

                // process first msg
                let serialised_msg = {
                    match first_frag.first() {
                        None => String::from_utf8(previous_fragment.clone()).unwrap(),
                        Some(&byte) if byte == init_delimiter => {
                            previous_fragment.append(&mut first_frag[1..].to_vec());
                            String::from_utf8(previous_fragment.clone()).unwrap()
                        }
                        Some(&_byte) => String::from_utf8(first_frag.to_vec()).unwrap(),
                    }
                };
                let first_msg = serde_json::from_str::<Message>(&serialised_msg).unwrap();
                tx.send(first_msg).await.unwrap();

                // process middle msgs
                let (last_frag, middle_binary_msgs) = match actual_fragments.split_last() {
                    Some(smt) => (*smt.0, smt.1),
                    None => continue 'process_reader,
                };

                // todo: here put a 'with_capacity' inherent to the buffer_len
                let mut middle_msgs = Vec::with_capacity(20);

                let _ = middle_binary_msgs.iter().map(|&raw_binary_msg| {
                    let binary_msg = match raw_binary_msg.first() {
                        None => panic!("This information unit should have a been complete but is of length 0!"),
                        Some(&ch) if ch == init_delimiter => raw_binary_msg[1..].to_vec(),
                        Some(&_ch) => panic!("This information unit should have should have started with the end delimiter but it does not!"),
                    };
                    let serialised_msg = String::from_utf8(binary_msg).unwrap();
                    let msg = serde_json::from_str(&serialised_msg).unwrap();
                    middle_msgs.push(msg);
                });

                let mut stream = tokio_stream::iter(middle_msgs);
                while let Some(msg) = stream.next().await {
                    tx.send(msg).await.unwrap();
                }

                // process last_msg and initialise the 'previous_fragment' for the next round
                if let Some(&ch) = buffer.last() {
                    if ch == end_delimiter {
                        previous_fragment.clear();
                        let serialised_msg = String::from_utf8(last_frag[1..].to_vec()).unwrap();
                        let msg = serde_json::from_str(&serialised_msg).unwrap();
                        tx.send(msg).await.unwrap();
                    } else {
                        *previous_fragment = last_frag.to_vec();
                        previous_fragment.reserve(BUFFER_LEN);
                        continue 'process_reader;
                    }
                } else {
                    panic!();
                }
            }
            Ok(_zero) => break 'process_reader,
        }
    }
}
