use serde::{Deserialize, Serialize};
use serde_json::error::Category;
use std::fmt;
use std::io::Write;
use std::string::ToString;

use thiserror::Error;

// ############################## CONSTANTS ##############################

pub mod constant {
    pub const SERVER_ADDR: &str = "127.0.0.1:6440";
    pub const MAX_NUM_USERS: usize = 2000;
}

// ############################## STRUCTS ##############################

/// This struct has only one purpose: ensure that the username is not longer than a certain length.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Username(String);

impl Username {
    /// Maximal length (in chars) of the username.
    pub const MAX_LEN: usize = 32;

    pub(crate) fn new(username_candidate: &str) -> Result<Username, TextValidityError> {
        if username_candidate.chars().count() > Username::MAX_LEN {
            return Err(TextValidityError::TooLong {
                kind_of_entity: "username".to_string(),
                actual_entity: username_candidate.to_string(),
                max_len: Username::MAX_LEN,
                actual_len: username_candidate.chars().count(),
            });
        }
        Ok(Username(username_candidate.to_string()))
    }

    pub fn choose() -> Username {
        let username = loop {
            print!(r###"Please input your desired userid and press "Enter": "###);
            if std::io::stdout().flush().is_err() {
                continue;
            }

            let mut username = String::new();

            if std::io::stdin().read_line(&mut username).is_err() {
                continue;
            }

            let username = username.trim().to_string();

            match Username::new(&username) {
                Ok(username) => break username,
                Err(err) => eprint_small_error(err),
            }
        };

        username
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Message is the type of packet unit that the client uses. They comprehend a username of at most
/// MAX_USERNAME_LEN chars and a content of at most MAX_CONTENT_LEN chars.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Message {
    username: Username,
    content: String,
}

impl Message {
    /// The delimiters to send serialised Messages in bytes through TCP connections. They are NOT appearing in any
    /// utf8 char as a byte (source: Wikipedia), thus the text does not need to be checked for their presence.
    pub(crate) const PAKET_INIT_U8: u8 = 0xC0;
    pub(crate) const PAKET_END_U8: u8 = 0xC1;

    /// Maximal length (in chars) of the content of the message. Keep in mind that a char is at most
    /// 4 byte long, and that a vector in Rust is at most (isize::MAX / 2) long. Since String is in
    /// reality a Vec of bytes, choices were made. The factor 100 is a security measure.
    pub const MAX_CONTENT_LEN: usize = {
        let max_char_len_string = 10000_usize;
        max_char_len_string / 2
    };

    /// The magic number 40 is here a rough estimate for the byte needed for the serde JSON
    /// encapsulation ('\n', '"', '{' '}' etc...). The '_LEN' constant must be multiplied for 4
    /// because that is the maximum length in bytes (u8) of a char in Rust. But if such char is a
    /// special one (e.g. '"'), then the serde library must put a '\' before it, and this means that
    /// in its serialised version there is 1 more byte, which means that each char is at most (4+1)
    /// bytes long.
    pub const MAX_PAKET_U8_LEN: usize =
        Message::MAX_CONTENT_LEN * (4 + 1) + Username::MAX_LEN * (4 + 1) + 40;

    pub(crate) const INCOMING_MSG_BUFFER_U8_LEN: usize = 1000_usize;
    pub(crate) const OUTGOING_MSG_BUFFER_U8_LEN: usize = 1000_usize;

    pub(crate) fn new(username: &Username, content: &str) -> Result<Message, TextValidityError> {
        assert!(Message::MAX_OFFICIAL_CONTENT_LEN < Message::MAX_CONTENT_LEN);

        if content.chars().count() > Message::MAX_CONTENT_LEN {
            return Err(TextValidityError::TooLong {
                kind_of_entity: "content".to_string(),
                actual_entity: content.to_string(),
                max_len: Message::MAX_CONTENT_LEN,
                actual_len: content.chars().count(),
            });
        }

        Ok(Message {
            username: username.clone(),
            content: content.to_string(),
        })
    }

    pub(crate) fn new_many(username: &Username, text: &str) -> Vec<Message> {
        let messages: Vec<Message> = text
            .chars()
            .collect::<Vec<char>>()
            .chunks(Message::MAX_CONTENT_LEN)
            // Justification 'unwrap': this can panic only if the content.len()>Message::MAX_CONTENT_LEN,
            // but this cannot happen because of the previous iterator adapter.
            .map(|content| Message::new(username, &content.iter().collect::<String>()).unwrap())
            .collect();
        messages
    }

    pub fn get_username(&self) -> Username {
        self.username.clone()
    }

    pub fn get_content(&self) -> String {
        self.content.clone()
    }

    pub fn paket(self) -> Vec<u8> {
        let serialized_in_bytes: Vec<u8> = serde_json::to_string(&self)
            // Justification 'unwrap': 'serde_json::to_string' fails if:
            // > "Serialization can fail if `T`'s implementation of `Serialize` decides to
            // > fail, or if `T` contains a map with non-string keys.".
            // Message contains only String(s) or wrapped String(s), thus this cannot fail.
            .unwrap()
            .as_bytes()
            .into_iter()
            .map(|&byte| byte)
            .collect();
        let paket: Vec<u8> = [Message::PAKET_INIT_U8]
            .into_iter()
            .chain(serialized_in_bytes)
            .chain([Message::PAKET_END_U8].into_iter())
            .collect();
        paket
    }

    pub fn from_paket(candidate: Vec<u8>) -> Result<Message, MsgFromPaketError> {
        match candidate.first() {
            Some(&byte) if byte == Message::PAKET_INIT_U8 => (),
            Some(&_byte) => return Err(MsgFromPaketError::NoInitDelimiter { candidate }),
            None => return Err(MsgFromPaketError::EmptyPaket),
        };

        match candidate.last() {
            Some(&byte) if byte == Message::PAKET_END_U8 => (),
            Some(&_byte) => return Err(MsgFromPaketError::NoEndDelimiter { candidate }),
            None => return Err(MsgFromPaketError::EmptyPaket),
        };

        let dispaketed = candidate[1..candidate.len() - 1].to_vec();
        let serialised = String::from_utf8(dispaketed)?;
        let message: Message = match serde_json::from_str(&serialised) {
            Ok(msg) => msg,
            Err(serde_json_err) => {
                let category = serde_json_err.classify();
                let io_error_kind = serde_json_err.io_error_kind();
                let (line, column) = (serde_json_err.line(), serde_json_err.column());
                return Err(MsgFromPaketError::SerdeJson {
                    dispaketed_candidate: serialised,
                    category,
                    io_error_kind,
                    line,
                    column,
                });
            }
        };
        Ok(message)
    }
}

pub enum UserStatus {
    Present,
    Absent,
}

// Justification 'unwrap': We expect that Message::MAX_CONTENT_LEN and Username::MAX_LEN should be
// big enough to write in a message official communications of this kind, and the only reason
// Message::new may fail is due to the length of its content.
/// This collection gathers the functions which create 'official' messages.
impl Message {
    pub const MAX_OFFICIAL_CONTENT_LEN: usize = 100;
    pub const MAX_OFFICIAL_PAKET_U8_LEN: usize =
        Message::MAX_OFFICIAL_CONTENT_LEN * (4 + 1) + Username::MAX_LEN * (4 + 1) + 40;

    pub fn craft_msg_helo(username: &Username) -> Message {
        let content = "helo".to_string();
        assert!(content.chars().count() <= Message::MAX_OFFICIAL_CONTENT_LEN);
        Message::new(username, &content).unwrap()
    }

    pub fn craft_msg_change_status(username: &Username, new_status: UserStatus) -> Message {
        let content = match new_status {
            UserStatus::Present => format!("{username} just joined the chat"),
            UserStatus::Absent => format!("{username} just left the chat"),
        };
        assert!(content.chars().count() <= Message::MAX_OFFICIAL_CONTENT_LEN);
        Message::new(&Username("SERVER".to_string()), &content).unwrap()
    }

    pub fn craft_msg_server_interrupt_connection() -> Message {
        let content = "The SERVER is shutting down this connection.".to_string();
        assert!(content.chars().count() <= Message::MAX_OFFICIAL_CONTENT_LEN);
        Message::new(&Username("SERVER".to_string()), &content).unwrap()
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum TextValidityError {
    #[error(r##"Too long {kind_of_entity} (expected at most {max_len} chars, found {actual_len}): [[{actual_entity}]]."##)]
    TooLong {
        kind_of_entity: String,
        actual_entity: String,
        max_len: usize,
        actual_len: usize,
    },
}

#[derive(Error, Debug, PartialEq)]
#[error(transparent)]
pub enum MsgFromPaketError {
    #[error(
        r##"No paket init byte delimiter ({}) found in the utf8 sample. The sample is: [[{candidate:?}]]."##,
        Message::PAKET_INIT_U8 as char
    )]
    NoInitDelimiter {
        candidate: Vec<u8>,
    },
    #[error(
        r##"No paket end byte delimiter ({}) found in the utf8 sample. The sample is: [[{candidate:?}]]."##,
        Message::PAKET_END_U8 as char
    )]
    NoEndDelimiter {
        candidate: Vec<u8>,
    },
    #[error(r##"serde_json Error: [[{dispaketed_candidate:?}]], {category:?}, {io_error_kind:?}, at coord (line, column)=({line},{column})."##)]
    SerdeJson {
        dispaketed_candidate: String,
        category: Category,
        io_error_kind: Option<std::io::ErrorKind>,
        line: usize,
        column: usize,
    },
    StringFromUtf8(#[from] std::string::FromUtf8Error),
    #[error(r##"The utf8 paket given was empty."##)]
    EmptyPaket,
}

/// Dispatch is the type of packet unit used exclusively by the server. It associates a byte vector
/// with an identifier, so that the clients can avoid to receive their own messages in cases of
/// multiple equal nicknames. The bytes are not assured to be a valid paket, even though this is auspicable.
#[derive(Debug, Clone)]
pub struct Dispatch {
    userid: usize,
    bytes: Vec<u8>,
}

impl Dispatch {
    pub fn new(userid: usize, bytes: Vec<u8>) -> Dispatch {
        Dispatch { userid, bytes }
    }

    pub fn get_userid(&self) -> usize {
        self.userid
    }

    pub fn get_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    pub fn into_msg(self) -> Result<Message, MsgFromPaketError> {
        Message::from_paket(self.bytes)
    }
}

pub fn eprint_small_error(err: impl std::error::Error) {
    eprintln!("##SmallError. Error: {}", err.to_string());
}

// ############################## TEST ##############################

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    #[test]
    fn test_text_validity() {
        let valid_username = craft_random_text_of_len(Username::MAX_LEN);
        let valid_content = craft_random_text_of_len(Message::MAX_CONTENT_LEN);

        let username =
            Username::new(&valid_username).expect("This username should have been valid");
        let valid_msg = Message::new(&username, &valid_content);
        assert!(valid_msg.is_ok());

        let too_long_username = craft_random_text_of_len(Message::MAX_CONTENT_LEN + 1);
        let too_long_content = craft_random_text_of_len(Message::MAX_CONTENT_LEN + 1);

        let invalid_username = Username::new(&too_long_username);
        assert!(invalid_username.is_err());

        let invalid_message = Message::new(&username, &too_long_content);
        assert!(invalid_message.is_err());
    }

    #[test]
    fn test_craft_random_valid_text_of_len() {
        for _ in 0..100 {
            let random_valid_text = craft_random_text_of_len(80);
            assert_eq!(random_valid_text.chars().count(), 80);
            // println!("Your random valid text is: [[[{random_valid_text}]]]");
        }
    }

    #[test]
    fn test_msg2paket2msg() {
        let valid_name = craft_random_text_of_len(Username::MAX_LEN);
        let valid_content = craft_random_text_of_len(Message::MAX_CONTENT_LEN);

        let valid_msg = Message::new(&Username::new(&valid_name).unwrap(), &valid_content)
            .expect("This message should have been valid!");
        let valid_paket = valid_msg.clone().paket();

        assert_eq!(Message::from_paket(valid_paket), Ok(valid_msg.clone()));

        let random_short_text = craft_random_text_of_len(Message::MAX_CONTENT_LEN);

        let candidate_no_init_delimiter: Vec<_> = serde_json::to_string(&valid_msg)
            .expect("The serde JSON serialization failed")
            .as_bytes()
            .into_iter()
            .chain(&[Message::PAKET_END_U8])
            .map(|&byte| byte)
            .collect();
        let candidate_no_end_delimiter: Vec<_> = [&Message::PAKET_INIT_U8]
            .into_iter()
            .chain(
                serde_json::to_string(&valid_msg)
                    .expect("The serde JSON serialization failed")
                    .as_bytes()
                    .iter(),
            )
            .map(|&byte| byte)
            .collect();
        let candidate_not_valid_serialised_msg: Vec<_> = [&Message::PAKET_INIT_U8]
            .into_iter()
            .chain(random_short_text.as_bytes())
            .chain([&Message::PAKET_END_U8].into_iter())
            .map(|&byte| byte)
            .collect::<Vec<_>>();

        assert_eq!(
            Message::from_paket(candidate_no_init_delimiter.clone()),
            Err(MsgFromPaketError::NoInitDelimiter {
                candidate: candidate_no_init_delimiter
            })
        );
        assert_eq!(
            Message::from_paket(candidate_no_end_delimiter.clone()),
            Err(MsgFromPaketError::NoEndDelimiter {
                candidate: candidate_no_end_delimiter
            })
        );
        assert_eq!(
            Message::from_paket(candidate_not_valid_serialised_msg),
            Err(MsgFromPaketError::SerdeJson {
                dispaketed_candidate: random_short_text,
                category: serde_json::error::Category::Syntax,
                io_error_kind: None,
                line: 1,
                column: 1
            })
        );
    }
}
