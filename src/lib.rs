use serde::{Deserialize, Serialize};
use serde_json::error::Category;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync,
};

use std::io::Write;

use tokio_stream::StreamExt;

use crate::MsgFromPaketError::SerdeJson;
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
    /// The delimiters to send serialised Messages in bytes through TCP connections. They are NOT appearing in any
    /// utf8 char as a byte (source: Wikipedia), thus the text does not need to be checked for their presence.
    pub(crate) const PAKET_INIT_U8: u8 = 0xC0;
    pub(crate) const PAKET_END_U8: u8 = 0xC1;

    /// Maximal length (in chars) of the username.
    pub const MAX_USERNAME_LEN: usize = 32;

    /// Maximal length (in chars) of the content of the message.
    pub const MAX_CONTENT_LEN: usize = 256;

    /// Maximal length of a paketed message (=JSON serialised message **in bytes** sorrounded by the <init/end> paket bytes)
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

    pub fn paket(self) -> Result<Vec<u8>, serde_json::Error> {
        let serialized_in_bytes: Vec<u8> = serde_json::to_string(&self)?
            .as_bytes()
            .into_iter()
            .map(|&byte| byte)
            .collect();
        let paket: Vec<u8> = [Message::PAKET_INIT_U8]
            .into_iter()
            .chain(serialized_in_bytes)
            .chain([Message::PAKET_END_U8].into_iter())
            .collect();
        Ok(paket)
    }

    fn from_paket(candidate: Vec<u8>) -> Result<Message, MsgFromPaketError> {
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
pub async fn message_depaketer(
    mut tcp_reader: impl AsyncRead + Unpin + Send + 'static,
    tx: sync::mpsc::Sender<Result<Message, MsgFromPaketError>>,
    // cancellation token
) -> Result<(), MsgDepaketerError> {
    let msgs_per_buffer = 10_usize;

    let mut buffer: Vec<u8> = Vec::with_capacity(Message::MAX_PAKETED_MSG_U8_LEN * msgs_per_buffer);
    let mut previous_fragment: Vec<u8> = Vec::with_capacity(Message::MAX_PAKETED_MSG_U8_LEN);

    'write_on_buffer: loop {
        buffer.clear();
        match tcp_reader.read_buf(&mut buffer).await {
            Ok(n) if n > 0 => {
                // Loop assumption: previous_fragment is initialised (eventually empty)

                let previous_fragment = &mut previous_fragment;
                let actual_fragments: Vec<&[u8]> = buffer
                    .split_inclusive(|&byte| byte == Message::PAKET_END_U8)
                    .collect();

                let (first_fragment, actual_fragments) = match actual_fragments.split_first() {
                    Some(smt) => (*smt.0, smt.1),
                    // todo: migliora questo errore
                    None => {
                        return Err(MsgDepaketerError::UnusualEmptyEntity {
                            entity: "buffer.split(end_delimiter).split_first()".to_string(),
                        })
                    }
                };

                let first_paket_candidate = previous_fragment
                    .iter()
                    .chain(first_fragment.iter())
                    .map(|&byte| byte)
                    .collect();
                let first_msg_result = Message::from_paket(first_paket_candidate);
                tx.send(first_msg_result).await?;

                let (last_fragment, middle_fragments) = match actual_fragments.split_last() {
                    Some(smt) => (*smt.0, smt.1),
                    None => continue 'write_on_buffer,
                };

                let mut stream = tokio_stream::iter(middle_fragments);
                while let Some(&fragment) = stream.next().await {
                    let msg_result = Message::from_paket(fragment.to_vec());
                    tx.send(msg_result).await?;
                }

                // The loop assumptions are fullfilled here
                match last_fragment.last() {
                    Some(&ch) if ch == Message::PAKET_END_U8 => {
                        let last_msg_result = Message::from_paket(last_fragment.to_vec());
                        tx.send(last_msg_result).await?;
                        previous_fragment.clear()
                    }
                    Some(&_ch) => *previous_fragment = last_fragment.to_vec(),
                    None => {
                        return Err(MsgDepaketerError::UnusualEmptyEntity {
                            entity: "last_fragment.last()".to_string(),
                        })
                    }
                }
            }
            Ok(_zero) => break 'write_on_buffer,
            Err(err) => return Err(MsgDepaketerError::Io(err)),
        }
    }
    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub enum MsgDepaketerError {
    Io(#[from] std::io::Error),
    TxSend(#[from] tokio::sync::mpsc::error::SendError<Result<Message, MsgFromPaketError>>),
    #[error("Unsual None returned from '{entity}' in 'handle_msgs_reader'.")]
    UnusualEmptyEntity {
        entity: String,
    },
}

// ############################## ASYNC WRITE FUNCTION ##############################

/// This function reads from the stdin with a 'prompt_buffer', stringify the result, packs it into
/// many different messages with the given username and sends them through the tcp_writer.
pub async fn message_paketer(
    mut stdin: impl AsyncRead + Unpin,
    mut tcp_writer: impl AsyncWrite + Unpin,
    username: &str,
    // cancellation_token: CancellationToken
) -> Result<(), MsgPaketerError> {
    let mut prompt_buffer: Vec<u8> = Vec::with_capacity(Message::MAX_CONTENT_LEN);
    let mut previous_prompt_fragment: Vec<u8> = Vec::with_capacity(3);

    'accepting_new_prompt: loop {
        print!("{username}:> ");
        std::io::stdout().flush()?;

        previous_prompt_fragment.clear();

        'processing_prompt_buffer: loop {
            // Loop assumption: previous_prompt_fragment is initialised (eventually empty)

            prompt_buffer.clear();

            match stdin.read_buf(&mut prompt_buffer).await {
                Ok(n) if n > 0 => {
                    let is_last_chunk = if prompt_buffer.last() == Some(&b'\n') {
                        prompt_buffer.pop();
                        true
                    } else {
                        false
                    };

                    if prompt_buffer.is_empty() {
                        continue 'accepting_new_prompt;
                    }

                    let chunk_to_process: Vec<u8> = previous_prompt_fragment
                        .iter()
                        .chain(prompt_buffer.iter())
                        .map(|&byte| byte)
                        .collect();

                    let text = match String::from_utf8(chunk_to_process.clone()) {
                        Ok(text) => {
                            previous_prompt_fragment.clear();
                            text
                        }
                        Err(error) => match error.utf8_error().error_len() {
                            None => {
                                let last_valid_index = error.utf8_error().valid_up_to();
                                let text_result = String::from_utf8(
                                    chunk_to_process[..last_valid_index].to_vec().clone(),
                                );

                                previous_prompt_fragment =
                                    chunk_to_process[last_valid_index..].to_vec();

                                text_result?
                            }
                            Some(_len) => return Err(MsgPaketerError::from(error)),
                        },
                    };

                    let msg_results: Vec<Result<Message, TextValidityError>> = text
                        .chars()
                        .collect::<Vec<char>>()
                        .chunks(Message::MAX_CONTENT_LEN)
                        .map(|char_slice| String::from_iter(char_slice))
                        .map(|content| Message::new(username, &content))
                        .collect();

                    let mut msg_result_stream = tokio_stream::iter(msg_results);

                    while let Some(msg_result) = msg_result_stream.next().await {
                        let paket = msg_result?.paket()?;
                        tcp_writer.write_all(&paket).await?;
                    }

                    if is_last_chunk {
                        continue 'accepting_new_prompt;
                    } else {
                        continue 'processing_prompt_buffer;
                    }
                }
                // todo: handle the transmission of 0 bytes with graceful shutdown. It is an
                // ambigous outcome: the tokio documentation claims this means an EOF and thus
                // that the reader could still return something, while the empirics tells me that
                // this happens only when the reader is unlinked from its source. I should verify.
                Ok(_null) => continue 'accepting_new_prompt,
                Err(error) => return Err(MsgPaketerError::from(error)),
            }
        }
    }

    // todo: remove this when adding graceful shutdown
    #[allow(unreachable_code)]
    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub enum MsgPaketerError {
    Io(#[from] std::io::Error),
    TxReceive(#[from] tokio::sync::mpsc::error::TryRecvError),
    #[error("Unsual None returned from '{entity}' in 'handle_msgs_reader'.")]
    UnusualEmptyEntity {
        entity: String,
    },
    StringFromUtf8(#[from] std::string::FromUtf8Error),
    TextValidity(#[from] TextValidityError),
    SerdeJsonError(#[from] serde_json::Error),
}

// ############################## TEST UTILITIES ##############################

pub mod test_util {
    use crate::*;

    pub fn craft_random_valid_text(char_length: usize) -> String {
        use rand::distr::{SampleString, StandardUniform};

        let random_string: String = StandardUniform.sample_string(&mut rand::rng(), char_length);
        //println!("random_valid_text: [[[{random_valid_text}]]]");

        random_string
    }

    pub fn craft_random_msg(username: &str) -> Message {
        let valid_text = craft_random_valid_text(Message::MAX_CONTENT_LEN);
        Message::new(username, &valid_text).expect("The random message construction failed.")
    }
}

// ############################## TEST ##############################

#[cfg(test)]
pub mod test {
    use crate::test_util::craft_random_valid_text;
    use crate::*;
    use tokio::net::TcpStream;
    use tokio_util::task::TaskTracker;

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
    }

    #[test]
    fn test_craft_random_valid_text() {
        for _ in 0..100 {
            let random_valid_text = test_util::craft_random_valid_text(80);
            assert_eq!(random_valid_text.chars().count(), 80);
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
    fn test_msg2paket2msg() {
        let valid_name = (1..=Message::MAX_USERNAME_LEN)
            .map(|_num| 'a')
            .collect::<String>();
        let valid_content = (1..=Message::MAX_CONTENT_LEN)
            .map(|_num| 'a')
            .collect::<String>();

        let valid_msg =
            Message::new(&valid_name, &valid_content).expect("The message is not valid");
        let valid_paket = valid_msg.clone().paket().expect("The paket is not valid");

        assert_eq!(Message::from_paket(valid_paket), Ok(valid_msg));

        let random_invalid_string_as_bytes: Vec<u8> =
            "aaaa".as_bytes().into_iter().map(|&byte| byte).collect();

        let candidate_no_init_delimiter: Vec<_> = random_invalid_string_as_bytes
            .clone()
            .into_iter()
            .chain([Message::PAKET_END_U8])
            .collect();
        let candidate_no_end_delimiter: Vec<_> = [Message::PAKET_INIT_U8]
            .into_iter()
            .chain(random_invalid_string_as_bytes.clone().into_iter())
            .collect();
        let candidate_not_valid_serialised_msg: Vec<u8> = [Message::PAKET_INIT_U8]
            .into_iter()
            .chain(random_invalid_string_as_bytes)
            .chain([Message::PAKET_END_U8].into_iter())
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
                dispaketed_candidate: "aaaa".to_string(),
                category: serde_json::error::Category::Syntax,
                io_error_kind: None,
                line: 1,
                column: 1
            })
        );
    }

    #[tokio::test]
    async fn test_message_depaketer() {
        let num_messages = 100;

        let paket: Vec<u8> = (0..num_messages)
            .map(|num| {
                let username = format!("user{}", num);
                let random_message = test_util::craft_random_msg(&username);
                random_message
                    .paket()
                    .expect("The paketing of a random message failed.")
            })
            .flatten()
            .collect();

        let reader = tokio_test::io::Builder::new().read(&paket).build();

        let (tx, mut rx) = tokio::sync::mpsc::channel(20);

        let task_tracker = TaskTracker::new();

        task_tracker.spawn(async move {
            message_depaketer(reader, tx)
                .await
                .expect("The message depaketer failed.");
            Ok::<(), MsgDepaketerError>(())
        });

        task_tracker.spawn(async move {
            while let Some(result) = rx.recv().await {
                // println!("result: {:?}", result);
                assert!(result.is_ok());

                if let Ok(_msg) = result {
                    // println!("{}:> {}", _msg.get_username(), _msg.get_content());
                }
            }
            Ok::<(), TextValidityError>(())
        });

        task_tracker.close();
        task_tracker.wait().await;
    }

    #[tokio::test]
    async fn test_message_paketer() {
        let num_messages_prompt = 100;

        let prompt_buffer0 = "Nam ignoratione rerum bonarum et malarum maxime
hominum vita vexatur ob eumque errorem et voluptatibus
maximis saepe privantur et durissimis animi doloribus
torquentur.
Ergo sapientia est adhibenda, quae et terroribus
cupiditatibusque detractis et omnium falsarum opinionum
temeritate derepta se nobis certissimam ducem praebeat ad
voluptatem\n"
            .to_string();

        let prompt_buffer1 = format!(
            "{}\n",
            craft_random_valid_text(Message::MAX_CONTENT_LEN * num_messages_prompt)
        );

        let prompt_buffer2 = (1..=num_messages_prompt).map(|num| -> Vec<char> {
            let prefix_len = "MESSAGE_n__[[[]]]\n".chars().count() + 1_usize + (num as f64).log10().floor() as usize;
            let msg = format!{"MESSAGE_n_{}_[[[{}]]]\n", num, craft_random_valid_text(Message::MAX_CONTENT_LEN - prefix_len)};
            msg.chars().collect()
        }).flatten().collect::<String>();

        let stdin = tokio_test::io::Builder::new()
            .read(prompt_buffer0.as_bytes())
            .read(prompt_buffer1.as_bytes())
            .read(prompt_buffer2.as_bytes())
            .build();

        let stdin = tokio::io::BufReader::new(stdin);

        let (tx_depaketer, mut rx_depaketer) =
            tokio::sync::mpsc::channel::<Result<Message, MsgFromPaketError>>(20);

        let test_task_tracker = TaskTracker::new();

        // server
        let server = test_task_tracker.spawn(async move {
            let listener = tokio::net::TcpListener::bind(constant::SERVER_ADDR)
                .await
                .expect("point0");

            let (socket_stream_server, _addr) = listener.accept().await.expect("point1");

            message_depaketer(socket_stream_server, tx_depaketer)
                .await
                .unwrap();
        });

        // client
        let client = test_task_tracker.spawn(async move {
            let socket_stream_client = TcpStream::connect(constant::SERVER_ADDR)
                .await
                .expect("point2");

            message_paketer(stdin, socket_stream_client, "peppino")
                .await
                .expect("I just exited the message paketer with an error");
        });

        while let Ok(msg) = rx_depaketer
            .recv()
            .await
            .expect("something went wrong in the message transmission")
        {
            println!("{}:> {}", msg.get_username(), msg.get_content());
        }

        // todo: add graceful shutdown in the test as well: a clean "Ok" for the test is auspicable.

        test_task_tracker.close();
        test_task_tracker.wait().await;

        if server.await.is_err() || client.await.is_err() {
            panic!("One of the two handles panicked");
        }
    }
}
