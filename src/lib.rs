use serde::{Deserialize, Serialize};
use serde_json::error::Category;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync,
};

use tokio_stream::StreamExt;

use crate::MsgFromUtf8PaketError::SerdeJson;
use thiserror::Error;

// ############################## CONSTANTS ##############################

pub mod constant {
    pub const SERVER_ADDR: &str = "127.0.0.1:6440";
    pub const MAX_NUM_USERS: usize = 2000;
}

// ############################## STRUCTS ##############################

/// Message is the type of packet unit that the client uses. They comprehend a username of at most
/// MAX_USERNAME_LEN chars and a content of at most MAX_CONTENT_LEN chars.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Message {
    username: String,
    content: String,
}

impl Message {
    /// The delimiters to send serialised Messages in TCP connections. They are required to be
    /// different from '{', '}', and to be different from each others.
    pub(crate) const TCP_INIT_DELIMITER_U8: u8 = b'|';
    pub(crate) const TCP_END_DELIMITER_U8: u8 = b'`';

    /// Maximal length (in chars) of the username.
    pub const MAX_USERNAME_LEN: usize = 32;

    /// Maximal length (in chars) of the content of the message.
    pub const MAX_CONTENT_LEN: usize = 256;

    /// Maximal length (in bytes) of a paketed message (=JSON serialised message sorrounded by the delimiters)
    ///
    /// The reasoning is that if the serialised message contains a special character (e.g. '{') at every
    /// (utf8-)char of the username and at every (utf8-)char of the content, and if such char
    /// is exactly 1 byte long, then the message will double its username and content u8_len, because the
    /// special char, in the serialization process, will be preceded by a '\' symbol (e.g. '\{').
    pub const MAX_PAKETED_MSG_U8_LEN: usize = {
        let delimiters_u8_len = 2_usize;

        // Rough estimate of "{\n""\n}, etc...
        let json_encapsulation_u8_len = 20_usize;

        let max_char_u8_len = 4_usize;
        let special_char_serialised_u8_len = max_char_u8_len * 2;

        let max_serialised_msg_u8_len = json_encapsulation_u8_len
            + (Message::MAX_USERNAME_LEN + Message::MAX_CONTENT_LEN)
                * special_char_serialised_u8_len;
        max_serialised_msg_u8_len + delimiters_u8_len
    };

    pub fn has_invalid_chars(text: &str) -> bool {
        assert_ne!(
            Message::TCP_INIT_DELIMITER_U8,
            Message::TCP_END_DELIMITER_U8
        );
        assert_ne!(Message::TCP_INIT_DELIMITER_U8, b'{');
        assert_ne!(Message::TCP_INIT_DELIMITER_U8, b'}');
        assert_ne!(Message::TCP_END_DELIMITER_U8, b'{');
        assert_ne!(Message::TCP_END_DELIMITER_U8, b'}');

        if text.contains(Message::TCP_INIT_DELIMITER_U8 as char)
            || text.contains(Message::TCP_END_DELIMITER_U8 as char)
        {
            true
        } else {
            false
        }
    }

    pub fn new(username: &str, content: &str) -> Result<Message, TextValidityError> {
        if username.chars().count() > Message::MAX_USERNAME_LEN {
            return Err(TextValidityError::TooLong {
                kind_of_entity: "username".to_string(),
                actual_entity: username.to_string(),
                max_len: Message::MAX_USERNAME_LEN,
                actual_len: username.chars().count(),
            });
        }

        if content.chars().count() > Message::MAX_CONTENT_LEN {
            return Err(TextValidityError::TooLong {
                kind_of_entity: "content".to_string(),
                actual_entity: content.to_string(),
                max_len: Message::MAX_CONTENT_LEN,
                actual_len: content.chars().count(),
            });
        }

        if Self::has_invalid_chars(username) {
            return Err(TextValidityError::InvalidChars {
                kind_of_entity: "username".to_string(),
                actual_entity: username.to_string(),
            });
        }

        if Self::has_invalid_chars(content) {
            return Err(TextValidityError::InvalidChars {
                kind_of_entity: "content".to_string(),
                actual_entity: content.to_string(),
            });
        }

        Ok(Message {
            username: username.to_string(),
            content: content.to_string(),
        })
    }

    pub fn many_try_new(username: &str, text: &str) -> Vec<Result<Message, TextValidityError>> {
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

    pub fn string_paket(self) -> Result<String, serde_json::Error> {
        let serialized = serde_json::to_string(&self)?;
        let paketed = format!(
            "{}{serialized}{}",
            Message::TCP_INIT_DELIMITER_U8 as char,
            Message::TCP_END_DELIMITER_U8 as char
        );
        Ok(paketed)
    }

    fn from_utf8_paket(candidate: Vec<u8>) -> Result<Message, MsgFromUtf8PaketError> {
        match candidate.first() {
            Some(&byte) if byte == Message::TCP_INIT_DELIMITER_U8 => (),
            Some(&_byte) => return Err(MsgFromUtf8PaketError::NoInitDelimiter { candidate }),
            None => return Err(MsgFromUtf8PaketError::EmptyPaket),
        };

        match candidate.last() {
            Some(&byte) if byte == Message::TCP_END_DELIMITER_U8 => (),
            Some(&_byte) => return Err(MsgFromUtf8PaketError::NoEndDelimiter { candidate }),
            None => return Err(MsgFromUtf8PaketError::EmptyPaket),
        };

        let dispaketed = candidate[1..candidate.len() - 1].to_vec();
        let serialised = String::from_utf8(dispaketed)?;
        let message: Message = match serde_json::from_str(&serialised) {
            Ok(msg) => msg,
            Err(serde_json_err) => {
                let category = serde_json_err.classify();
                let io_error_kind = serde_json_err.io_error_kind();
                let (line, column) = (serde_json_err.line(), serde_json_err.column());
                return Err(SerdeJson {
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

#[derive(Error, Debug, PartialEq)]
pub enum TextValidityError {
    #[error(r##"Too long {kind_of_entity} (expected at most {max_len} chars, found {actual_len}): [[{actual_entity}]]."##)]
    TooLong {
        kind_of_entity: String,
        actual_entity: String,
        max_len: usize,
        actual_len: usize,
    },
    #[error(
        r##"Invalid chars found in {kind_of_entity} (invalid chars: '{}','{}'): [[{actual_entity}]]."##,
        Message::TCP_INIT_DELIMITER_U8 as char,
        Message::TCP_END_DELIMITER_U8 as char
    )]
    InvalidChars {
        kind_of_entity: String,
        actual_entity: String,
    },
}

#[derive(Error, Debug, PartialEq)]
#[error(transparent)]
pub enum MsgFromUtf8PaketError {
    #[error(
        r##"No init delimiter ({}) found in the utf8 sample. The sample is: [[{candidate:?}]]."##,
        Message::TCP_INIT_DELIMITER_U8 as char
    )]
    NoInitDelimiter {
        candidate: Vec<u8>,
    },
    #[error(
        r##"No end delimiter ({}) found in the utf8 sample. The sample is: [[{candidate:?}]]."##,
        Message::TCP_END_DELIMITER_U8 as char
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

// ############################## ASYNC READ FUNCTION ##############################

/// This function reads messages from a Reader and forward them through its channel.
pub async fn handle_msgs_reader(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    tx: sync::mpsc::Sender<Result<Message, MsgFromUtf8PaketError>>,
    // cancellation token
) -> Result<(), MsgReaderError> {
    let msgs_per_buffer = 10_usize;

    let mut buffer: Vec<u8> = Vec::with_capacity(Message::MAX_PAKETED_MSG_U8_LEN * msgs_per_buffer);
    let mut previous_fragment: Vec<u8> = Vec::with_capacity(Message::MAX_PAKETED_MSG_U8_LEN);

    'write_on_buffer: loop {
        buffer.clear();
        match reader.read_buf(&mut buffer).await {
            Ok(n) if n > 0 => {
                // Loop assumption: previous_fragment is initialised (eventually empty)

                let previous_fragment = &mut previous_fragment;
                let actual_fragments: Vec<&[u8]> = buffer
                    .split_inclusive(|&byte| byte == Message::TCP_END_DELIMITER_U8)
                    .collect();

                let (first_fragment, actual_fragments) = match actual_fragments.split_first() {
                    Some(smt) => (*smt.0, smt.1),
                    // todo: migliora questo errore
                    None => {
                        return Err(MsgReaderError::UnusualEmptyEntity {
                            entity: "buffer.split(end_delimiter).split_first()".to_string(),
                        })
                    }
                };

                let first_paket_candidate = previous_fragment
                    .iter()
                    .chain(first_fragment.iter())
                    .map(|&byte| byte)
                    .collect();
                let first_msg_result = Message::from_utf8_paket(first_paket_candidate);
                tx.send(first_msg_result).await?;

                let (last_fragment, middle_fragments) = match actual_fragments.split_last() {
                    Some(smt) => (*smt.0, smt.1),
                    None => continue 'write_on_buffer,
                };

                let mut stream = tokio_stream::iter(middle_fragments);
                while let Some(&fragment) = stream.next().await {
                    let msg_result = Message::from_utf8_paket(fragment.to_vec());
                    tx.send(msg_result).await?;
                }

                // The loop assumptions are fullfilled here
                match last_fragment.last() {
                    Some(&ch) if ch == Message::TCP_END_DELIMITER_U8 => {
                        let last_msg_result = Message::from_utf8_paket(last_fragment.to_vec());
                        tx.send(last_msg_result).await?;
                        previous_fragment.clear()
                    }
                    Some(&_ch) => *previous_fragment = last_fragment.to_vec(),
                    None => {
                        return Err(MsgReaderError::UnusualEmptyEntity {
                            entity: "last_fragment.last()".to_string(),
                        })
                    }
                }
            }
            Ok(_zero) => break 'write_on_buffer,
            Err(err) => return Err(MsgReaderError::Io(err)),
        }
    }
    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub enum MsgReaderError {
    Io(#[from] std::io::Error),
    TxSend(#[from] tokio::sync::mpsc::error::SendError<Result<Message, MsgFromUtf8PaketError>>),
    #[error("Unsual None returned from '{entity}' in 'handle_msgs_reader'.")]
    UnusualEmptyEntity {
        entity: String,
    },
}

pub mod test_util {
    use rand::Rng;

    use crate::*;

    pub fn craft_random_valid_text(char_length: usize) -> String {
        use rand::distr::{SampleString, StandardUniform};

        let random_string: String = StandardUniform.sample_string(&mut rand::rng(), char_length);

        let random_valid_text: String = random_string
            .chars()
            .map(|mut ch| {
                while ch == Message::TCP_INIT_DELIMITER_U8 as char
                    || ch == Message::TCP_END_DELIMITER_U8 as char
                {
                    ch = rand::rng().sample(StandardUniform);
                }
                ch
            })
            .collect();
        //println!("random_valid_text: [[[{random_valid_text}]]]");

        random_valid_text
    }

    pub fn craft_random_msg(username: &str) -> Message {
        let valid_text = craft_random_valid_text(Message::MAX_CONTENT_LEN);
        Message::new(username, &valid_text).expect("The random message construction failed.")
    }
}

// ############################## TEST ##############################

#[cfg(test)]
pub mod test {
    use crate::*;

    #[test]
    fn test_text_validity() {
        let valid_name = (1..=Message::MAX_USERNAME_LEN)
            .map(|_num| 'a')
            .collect::<String>();
        let valid_content = (1..=Message::MAX_CONTENT_LEN)
            .map(|_num| 'a')
            .collect::<String>();

        let too_long_name = (1..=Message::MAX_USERNAME_LEN + 1)
            .map(|_num| 'a')
            .collect::<String>();
        let too_long_content = (1..=Message::MAX_CONTENT_LEN + 1)
            .map(|_num| 'a')
            .collect::<String>();

        let invalid_chars_text_1 = format!("{}", Message::TCP_INIT_DELIMITER_U8 as char);
        let invalid_chars_text_2 = format!("{}", Message::TCP_END_DELIMITER_U8 as char);

        assert_eq!(
            Message::new(&valid_name, &valid_content),
            Ok(Message {
                username: valid_name.clone(),
                content: valid_content.clone()
            })
        );
        assert_eq!(
            Message::new(&valid_name, &too_long_content),
            Err(TextValidityError::TooLong {
                kind_of_entity: "content".to_string(),
                actual_entity: too_long_content.to_string(),
                max_len: Message::MAX_CONTENT_LEN,
                actual_len: too_long_content.chars().count()
            })
        );
        assert_eq!(
            Message::new(&too_long_name, &valid_content),
            Err(TextValidityError::TooLong {
                kind_of_entity: "username".to_string(),
                actual_entity: too_long_name.to_string(),
                max_len: Message::MAX_USERNAME_LEN,
                actual_len: too_long_name.chars().count()
            })
        );

        assert_eq!(Message::has_invalid_chars(&invalid_chars_text_1), true);
        assert_eq!(Message::has_invalid_chars(&invalid_chars_text_2), true);
        assert_eq!(Message::has_invalid_chars(&valid_name), false);
    }

    #[test]
    fn test_craft_random_valid_text() {
        for _ in 0..100 {
            let random_valid_text = test_util::craft_random_valid_text(80);
            assert_eq!(random_valid_text.chars().count(), 80);
            assert_eq!(Message::has_invalid_chars(&random_valid_text), false);
            // println!("Your random valid text is: [[[{random_valid_text}]]]");
        }
    }

    #[test]
    fn test_craft_random_msg() {
        for _ in 0..100 {
            let random_username = test_util::craft_random_valid_text(Message::MAX_USERNAME_LEN);
            let _random_msg_result = test_util::craft_random_msg(&random_username);

            // println!("msg_result: [[[{:?}]]]", _random_msg_result);
        }
    }

    #[test]
    fn utf8paket2msg() -> Result<(), Box<dyn std::error::Error>> {
        let valid_name = (1..=Message::MAX_USERNAME_LEN)
            .map(|_num| 'a')
            .collect::<String>();
        let valid_content = (1..=Message::MAX_CONTENT_LEN)
            .map(|_num| 'a')
            .collect::<String>();

        let valid_msg = Message::new(&valid_name, &valid_content)?;
        let valid_utf8_paket: Vec<_> = valid_msg.clone().string_paket()?.bytes().collect();

        assert_eq!(Message::from_utf8_paket(valid_utf8_paket), Ok(valid_msg));

        let candidate_no_init_delimiter: Vec<_> =
            format!("aaaaaa{}", Message::TCP_END_DELIMITER_U8 as char)
                .bytes()
                .collect();
        let candidate_no_end_delimiter: Vec<_> =
            format!("{}aaaaaa", Message::TCP_INIT_DELIMITER_U8 as char)
                .bytes()
                .collect();
        let candidate_not_valid_serialised_msg: Vec<_> = format!(
            "{}aaaa{}",
            Message::TCP_INIT_DELIMITER_U8 as char,
            Message::TCP_END_DELIMITER_U8 as char
        )
        .bytes()
        .collect();

        assert_eq!(
            Message::from_utf8_paket(candidate_no_init_delimiter.clone()),
            Err(MsgFromUtf8PaketError::NoInitDelimiter {
                candidate: candidate_no_init_delimiter
            })
        );
        assert_eq!(
            Message::from_utf8_paket(candidate_no_end_delimiter.clone()),
            Err(MsgFromUtf8PaketError::NoEndDelimiter {
                candidate: candidate_no_end_delimiter
            })
        );
        assert_eq!(
            Message::from_utf8_paket(candidate_not_valid_serialised_msg),
            Err(MsgFromUtf8PaketError::SerdeJson {
                dispaketed_candidate: "aaaa".to_string(),
                category: serde_json::error::Category::Syntax,
                io_error_kind: None,
                line: 1,
                column: 1
            })
        );
        Ok(())
    }
}
