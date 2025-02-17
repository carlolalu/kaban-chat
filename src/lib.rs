use serde::{Deserialize, Serialize};
use serde_json::error::Category;
use std::fmt;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync,
};

use std::io::Write;
use std::string::ToString;

use tokio_stream::StreamExt;

use crate::MsgFromPaketError::SerdeJson;
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

    fn new(username_candidate: &str) -> Result<Username, TextValidityError> {
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
    /// encapsulation ('\n', '"', '{' '}' etc...)
    pub const MAX_PAKET_U8_LEN: usize = Message::MAX_CONTENT_LEN * 4 + Username::MAX_LEN * 4 + 40;

    pub(crate) const INCOMING_MSG_BUFFER_U8_LEN: usize = 1000_usize;
    pub(crate) const OUTGOING_MSG_BUFFER_U8_LEN: usize = 1000_usize;

    pub(crate) fn new(username: &Username, content: &str) -> Result<Message, TextValidityError> {
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
            .expect("The conversion of Message to its serialised JSON version failed.")
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

pub enum UserStatus {
    Present,
    Absent,
}

/// This collection gathers the functions which create 'official' messages
impl Message {
    pub fn helo_msg(username: &Username) -> Message {
        Message::new(username, "helo")
            .expect("The 'helo' message could not be crafted because its content was too long")
    }

    pub fn change_status(username: &Username, new_status: UserStatus) -> Message {
        let content = match new_status {
            UserStatus::Present => format!("{username} just joined the chat"),
            UserStatus::Absent => format!("{username} just left the chat"),
        };
        Message::new(&Username("SERVER".to_string()), &content).expect(
            "The 'status_change' message  could not be crafted because its content was too long",
        )
    }

    pub fn craft_server_interrupt_connection() -> Message {
        let content = "The SERVER is shutting down this connection.".to_string();
        Message::new(&Username("SERVER".to_string()), &content).expect("The 'server_interrupt_connection' message  could not be crafted because its content was too long")
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

// ############################## ASYNC READ AND WRITE FUNCTIONS ##############################

/// This function reads pakets from a Reader and forward them through its channel.
pub async fn pakets_extractor(
    mut tcp_reader: impl AsyncRead + Unpin + Send + 'static,
    tx: sync::mpsc::Sender<Vec<u8>>,
    // cancellation token
) -> Result<(), MsgDepaketerError> {
    let mut buffer: Vec<u8> = Vec::with_capacity(Message::INCOMING_MSG_BUFFER_U8_LEN);
    let mut previous_fragment: Vec<u8> = Vec::with_capacity(Message::INCOMING_MSG_BUFFER_U8_LEN);

    'write_on_buffer: loop {
        buffer.clear();
        match tcp_reader.read_buf(&mut buffer).await {
            Ok(n) if n > 0 => {
                // Loop assumption: previous_fragment is initialised (eventually empty)

                let previous_fragment = &mut previous_fragment;
                let actual_fragments: Vec<&[u8]> = buffer
                    .split_inclusive(|&byte| byte == Message::PAKET_END_U8)
                    .collect();

                let (first_fragment, subsequent_fragments) = match actual_fragments.split_first() {
                    Some(smt) => (*smt.0, smt.1),
                    None => {
                        let err = UnusualEmptyEntity {
                            entity: "actual_fragments".to_string(),
                        };
                        eprint_small_error(err);
                        continue 'write_on_buffer;
                    }
                };

                if Some(&Message::PAKET_END_U8) == first_fragment.last() {
                    let first_paket_candidate = previous_fragment
                        .iter()
                        .chain(first_fragment.iter())
                        .map(|&byte| byte)
                        .collect();

                    tx.send(first_paket_candidate).await?;

                    let (last_fragment, middle_fragments) = match subsequent_fragments.split_last()
                    {
                        Some(smt) => (*smt.0, smt.1),
                        None => continue 'write_on_buffer,
                    };

                    let mut stream = tokio_stream::iter(middle_fragments);
                    while let Some(&fragment) = stream.next().await {
                        tx.send(fragment.to_vec()).await?;
                    }

                    // The loop assumptions are fulfilled here
                    match last_fragment.last() {
                        Some(&ch) if ch == Message::PAKET_END_U8 => {
                            tx.send(last_fragment.to_vec()).await?;
                            previous_fragment.clear()
                        }
                        Some(&_ch) => *previous_fragment = last_fragment.to_vec(),
                        None => {
                            let err = UnusualEmptyEntity {
                                entity: "last_fragment".to_string(),
                            };
                            eprint_small_error(err);
                            continue 'write_on_buffer;
                        }
                    }
                } else {
                    previous_fragment.append(&mut first_fragment.to_vec());

                    if previous_fragment.len() > Message::MAX_PAKET_U8_LEN {
                        let err = MsgDepaketerError::TooLongPaket {
                            paket_u8_len: previous_fragment.len(),
                        };
                        eprint_small_error(err);
                        previous_fragment.clear()
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
    TxSend(#[from] tokio::sync::mpsc::error::SendError<Vec<u8>>),
    #[error("A too long paket arrived, it had {paket_u8_len} bytes. Such paket will be dropped.")]
    TooLongPaket {
        paket_u8_len: usize,
    },
}

/// This function reads from the stdin with a 'prompt_buffer', stringify the result, packs it into
/// many different messages with the given username and sends them through the tcp_writer.
pub async fn stdin2tcp(
    mut stdin: impl AsyncRead + Unpin,
    mut tcp_writer: impl AsyncWrite + Unpin,
    username: &Username,
    // cancellation_token: CancellationToken
) -> Result<(), MsgPaketerError> {
    let mut prompt_buffer: Vec<u8> = Vec::with_capacity(Message::OUTGOING_MSG_BUFFER_U8_LEN);
    let mut prompt: Vec<u8> = Vec::with_capacity(Message::OUTGOING_MSG_BUFFER_U8_LEN * 30);

    let prompt = &mut prompt;

    'accepting_new_prompt: loop {
        print!("{username}:> ");
        match std::io::stdout().flush() {
            Ok(()) => (),
            Err(err) => {
                eprint_small_error(err);
                continue 'accepting_new_prompt;
            }
        }

        prompt.clear();

        // Build a Vec<String> where each String has at most Message::MAX_CONTENT_LEN chars
        'stringigy_prompt_buffer: loop {
            prompt_buffer.clear();

            match stdin.read_buf(&mut prompt_buffer).await {
                Ok(n) if n > 0 => {
                    let is_last_chunk = if prompt_buffer.last() == Some(&b'\n') {
                        prompt_buffer.pop();
                        true
                    } else {
                        false
                    };

                    *prompt = prompt
                        .iter()
                        .chain(prompt_buffer.iter())
                        .map(|&byte| byte)
                        .collect();

                    if is_last_chunk || prompt_buffer.is_empty() {
                        break 'stringigy_prompt_buffer;
                    }
                }
                // todo: handle the transmission of 0 bytes with graceful shutdown. It is an
                // ambigous outcome: the tokio documentation claims this means an EOF and thus
                // that the reader could still return something, while the empirics tells me that
                // this happens only when the reader is unlinked from its source. I should verify.
                Ok(_null) => continue 'stringigy_prompt_buffer,
                Err(error) => return Err(MsgPaketerError::from(error)),
            }
        }

        // todo: avoid the '.unwrap()' and treat the errors singularly
        // which the prompt is longer than the maximum String length
        let text = String::from_utf8(prompt.to_vec()).unwrap();
        let pakets: Vec<u8> = Message::new_many(username, &text)
            .into_iter()
            .map(|msg| msg.paket())
            .flatten()
            .collect();
        tcp_writer.write_all(&pakets).await?;
    }

    // remove this when adding graceful shutdown
    #[allow(unreachable_code)]
    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub enum MsgPaketerError {
    Io(#[from] std::io::Error),
    TxReceive(#[from] tokio::sync::mpsc::error::TryRecvError),
}

#[derive(Error, Debug)]
#[error("Unusual None returned from '{entity}' in 'message_paketer'.")]
pub struct UnusualEmptyEntity {
    entity: String,
}

pub fn eprint_small_error(err: impl std::error::Error) {
    eprintln!("##SmallError. Error: {}", err.to_string());
}

// ############################## TEST UTILITIES ##############################

pub mod test_util {
    use crate::*;

    pub fn craft_random_text_of_len(char_length: usize) -> String {
        use rand::distr::{SampleString, StandardUniform};

        let random_string: String = StandardUniform.sample_string(&mut rand::rng(), char_length);
        //println!("random_valid_text: [[[{random_valid_text}]]]");

        random_string
    }

    pub fn craft_random_text() -> String {
        let random_len = rand::random_range(0..(isize::MAX as usize));
        let random_valid_text = craft_random_text_of_len(random_len);

        //println!("random_valid_text: [[[{random_valid_text}]]]");

        random_valid_text
    }

    pub fn craft_random_msg(username: &Username) -> Message {
        let random_msg_len = rand::random_range(0..Message::MAX_CONTENT_LEN);
        let valid_text = craft_random_text_of_len(random_msg_len);
        Message::new(username, &valid_text).expect("The random message construction failed.")
    }
}

// ############################## TEST ##############################

#[cfg(test)]
pub mod test {
    use crate::test_util::*;
    use crate::*;
    use tokio::net::TcpStream;
    use tokio_util::task::TaskTracker;

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
            let random_valid_text = test_util::craft_random_text_of_len(80);
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

    #[tokio::test]
    async fn test_pakets_extractor() {
        let num_messages = 100;

        let paket: Vec<u8> = (0..num_messages)
            .map(|num| {
                let username = Username::new(&format!("user{}", num)).expect("This username is not valid");
                let random_message = test_util::craft_random_msg(&username);
                random_message.paket()
            })
            .flatten()
            .collect();

        let reader = tokio_test::io::Builder::new().read(&paket).build();

        let (tx, mut rx) = tokio::sync::mpsc::channel(20);

        let task_tracker = TaskTracker::new();
        let mut handles = Vec::new();

        handles.push( task_tracker.spawn(async move {
            pakets_extractor(reader, tx)
                .await
                .expect("The message depaketer failed.");
        }));

        handles.push( task_tracker.spawn(async move {
            while let Some(paket) = rx.recv().await {
                let message_result = Message::from_paket(paket);
                assert!(message_result.is_ok());
                if let Ok(_msg) = message_result {
                    // println!("{}:> {}", _msg.get_username(), _msg.get_content());
                }
            }
        }));

        task_tracker.close();
        task_tracker.wait().await;

        for handle in handles {
            if handle.await.is_err() { panic!("One of hte tasks panicked!")}
        }
    }

    #[tokio::test]
    async fn test_stdin2tcp() {
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
            craft_random_text_of_len(Message::MAX_CONTENT_LEN)
        );

        let prompt_buffer2 = (1..=num_messages_prompt).map(|num| -> Vec<char> {
            let prefix_len = "MESSAGE_n__[[[]]]\n".chars().count() + 1_usize + (num as f64).log10().floor() as usize;
            let msg = format!{"MESSAGE_n_{}_[[[{}]]]\n", num, craft_random_text_of_len(Message::MAX_CONTENT_LEN - prefix_len)};
            msg.chars().collect()
        }).flatten().collect::<String>();

        let stdin = tokio_test::io::Builder::new()
            .read(prompt_buffer0.as_bytes())
            .read(prompt_buffer1.as_bytes())
            .read(prompt_buffer2.as_bytes())
            .build();

        let stdin = tokio::io::BufReader::new(stdin);

        let (tx_depaketer, mut rx_depaketer) = tokio::sync::mpsc::channel::<Vec<u8>>(20);

        let test_task_tracker = TaskTracker::new();

        // server
        let server = test_task_tracker.spawn(async move {
            let listener = tokio::net::TcpListener::bind(constant::SERVER_ADDR)
                .await
                .expect("point0");

            let (socket_stream_server, _addr) = listener.accept().await.expect("point1");

            pakets_extractor(socket_stream_server, tx_depaketer)
                .await
                .unwrap();
        });

        // client
        let client = test_task_tracker.spawn(async move {
            let socket_stream_client = TcpStream::connect(constant::SERVER_ADDR)
                .await
                .expect("point2");

            stdin2tcp(
                stdin,
                socket_stream_client,
                &Username::new("peppino").unwrap(),
            )
            .await
            .expect("I just exited the message paketer with an error");
        });

        let messages = test_task_tracker.spawn(async move {
            loop {
                let paket = rx_depaketer
                    .recv()
                    .await
                    .expect("something went wrong in the message transmission");

                let msg_result = Message::from_paket(paket);
                assert!(msg_result.is_ok());
                if let Ok(_msg) = msg_result {
                    // println!("{}:> {}", _msg.get_username(), _msg.get_content());
                }
            }
        });
        // todo: add graceful shutdown in the test as well: a clean "Ok" for the test is auspicable.

        test_task_tracker.close();
        test_task_tracker.wait().await;

        if server.await.is_err() || client.await.is_err() || messages.await.is_err() {
            panic!("One of the two handles panicked");
        }
    }
}
