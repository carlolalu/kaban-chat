use crate::prelude::*;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync,
};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

// ############################## ASYNC READ AND WRITE FUNCTIONS ##############################

/// This function reads pakets from a Reader and forward them through its channel.
pub async fn pakets_extractor(
    mut tcp_reader: impl AsyncRead + Unpin + Send + 'static,
    tx: sync::mpsc::Sender<Vec<u8>>,
    cancellation_token: CancellationToken,
) -> Result<(), PaketsExtractorError> {
    let mut buffer: Vec<u8> = Vec::with_capacity(Message::INCOMING_MSG_BUFFER_U8_LEN);
    let mut previous_fragment: Vec<u8> = Vec::with_capacity(Message::INCOMING_MSG_BUFFER_U8_LEN);

    let outcome = 'writing_on_buffer: loop {
        buffer.clear();

        let result_tcp_read = tokio::select! {
            result_tcp_read = tcp_reader.read_buf(&mut buffer) => result_tcp_read,
            _cancellation = cancellation_token.cancelled() => break 'writing_on_buffer Ok(()),
        };

        match result_tcp_read {
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
                        continue 'writing_on_buffer;
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
                        None => continue 'writing_on_buffer,
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
                            continue 'writing_on_buffer;
                        }
                    }
                } else {
                    previous_fragment.append(&mut first_fragment.to_vec());

                    if previous_fragment.len() > Message::MAX_PAKET_U8_LEN {
                        let err = PaketsExtractorError::TooLongPaket {
                            paket_u8_len: previous_fragment.len(),
                        };
                        eprint_small_error(err);

                        previous_fragment.clear();
                        'reinitialising_previous_fragment: loop {
                            buffer.clear();
                            match tcp_reader.read_buf(&mut buffer).await {
                                Ok(n) if n > 0 => {
                                    if buffer.contains(&Message::PAKET_END_U8) {
                                        let fragments: Vec<&[u8]> = buffer
                                            .splitn(2, |&byte| byte == Message::PAKET_END_U8)
                                            .collect();
                                        *previous_fragment = match fragments.get(1) {
                                            Some(&slice) => slice.to_vec(),
                                            None => {
                                                let err = UnusualEmptyEntity {
                                                    entity: "slice in the loop <'reinitialising_previous_fragment>".to_string(),
                                                };
                                                eprint_small_error(err);
                                                continue 'writing_on_buffer;
                                            }
                                        }
                                    } else {
                                        continue 'reinitialising_previous_fragment;
                                    }
                                }
                                Ok(_zero) => continue 'writing_on_buffer,
                                Err(err) => {
                                    break 'writing_on_buffer Err(PaketsExtractorError::Io(err))
                                }
                            }
                        }
                    }
                }
            }
            Ok(_zero) => break 'writing_on_buffer Err(PaketsExtractorError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "The tcp reader read 0 bytes: the connection was interrupted."))),
            Err(err) => break 'writing_on_buffer Err(PaketsExtractorError::Io(err)),
        }
    };

    cancellation_token.cancel();

    outcome
}

#[derive(Error, Debug)]
#[error(transparent)]
pub enum PaketsExtractorError {
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
    cancellation_token: CancellationToken,
) -> Result<(), Stdin2TcpError> {
    let mut prompt_buffer: Vec<u8> = Vec::with_capacity(Message::OUTGOING_MSG_BUFFER_U8_LEN);
    let mut prompt: Vec<u8> = Vec::with_capacity(Message::OUTGOING_MSG_BUFFER_U8_LEN * 30);
    let mut stdout = tokio::io::stdout();
    let username_prompt = format!("{username}:> ");

    let prompt = &mut prompt;

    'accepting_new_prompt: loop {
        tokio::select! {
            result_prompt = stdout.write_all(username_prompt.as_bytes()) => {
                match result_prompt {
                    Ok(()) => (),
                    Err(err) => {
                        eprint_small_error(err);
                        continue 'accepting_new_prompt;
                    },
                }
            },
            _cancellation = cancellation_token.cancelled() => break 'accepting_new_prompt,
        }

        prompt.clear();

        // Build a Vec<String> where each String has at most Message::MAX_CONTENT_LEN chars
        'gathering_prompt_buffer: loop {
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
                        break 'gathering_prompt_buffer;
                    }
                }
                Ok(_zero) => break 'gathering_prompt_buffer,
                Err(error) => return Err(Stdin2TcpError::from(error)),
            }
        }

        if prompt.is_empty() {
            continue 'accepting_new_prompt;
        }

        let text = match String::from_utf8(prompt.to_vec()) {
            Ok(text) => text,
            Err(err) => {
                eprint_small_error(err);
                continue 'accepting_new_prompt;
            }
        };

        let pakets: Vec<u8> = Message::new_many(username, &text)
            .into_iter()
            .map(|msg| msg.paket())
            .flatten()
            .collect();
        tcp_writer.write_all(&pakets).await?;
    }

    cancellation_token.cancel();

    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub enum Stdin2TcpError {
    Io(#[from] std::io::Error),
    TxReceive(#[from] tokio::sync::mpsc::error::TryRecvError),
}

#[derive(Error, Debug)]
#[error("Unusual None returned from '{entity}' in 'message_paketer'.")]
pub struct UnusualEmptyEntity {
    entity: String,
}

// ############################## TEST ##############################

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use tokio::net::TcpStream;
    use tokio::time::Duration;
    use tokio_util::sync::CancellationToken;
    use tokio_util::task::TaskTracker;

    #[tokio::test]
    async fn test_pakets_extractor() {
        let num_messages = 100;
        let mut msg_received = 0;

        let pakets: Vec<u8> = (0..num_messages)
            .map(|num| {
                let username =
                    Username::new(&format!("user{}", num)).expect("This username is not valid");
                let prefix_len = "MESSAGE_n__[[[]]]\n".chars().count() + 1_usize + (num as f64).log10().floor() as usize;
                // Sometimes the test fails for a too long paket. The frequency of this happening
                // seems to drop when the length of these messages decreases.
                let content = format!{"MESSAGE_n_{}_[[[{}]]]\n", num, craft_random_text_of_len(Message::MAX_CONTENT_LEN - prefix_len)};
                let paket = Message::new(&username, &content).unwrap().paket();
                paket
            })
            .flatten()
            .collect();
        let reader = tokio_test::io::Builder::new().read(&pakets).build();

        let (tx, rx) = tokio::sync::mpsc::channel(20);
        let rx = std::sync::Arc::new(tokio::sync::Mutex::new(rx));

        let cancellation_token = CancellationToken::new();
        let cancellation_token_check = cancellation_token.clone();
        let cancellation_token_control = cancellation_token.clone();

        let task_tracker = TaskTracker::new();

        let handle_pakets_extractor = task_tracker.spawn(async move {
            assert!(
                pakets_extractor(reader, tx, cancellation_token)
                    .await
                    .is_ok(),
                "The pakets_extractor yielded an error."
            );
            assert!(
                cancellation_token_check.is_cancelled(),
                "The paket extractor died before the cancellation token was cancelled."
            );
        });

        let handle_pakets_receiver = task_tracker.spawn(async move {
            let rx = rx.clone();

            let begin_recv = tokio::time::Instant::now();

            loop {
                if let Some(paket) = rx.lock().await.recv().await {
                    let message_result = Message::from_paket(paket);
                    assert!(
                        message_result.is_ok(),
                        "The conversion from paket to message failed"
                    );
                    if let Ok(_msg) = message_result {
                        msg_received += 1;
                        // println!("{}:> {}", _msg.get_username(), _msg.get_content());
                    }

                    if msg_received == num_messages {
                        break;
                    } else if tokio::time::Instant::now().duration_since(begin_recv)
                        > Duration::from_secs(3)
                    {
                        panic!("1 second passed and yet all messages are to arrive.")
                    }
                }
            }

            assert_eq!(
                num_messages, msg_received,
                "Not all messages were received."
            );
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(
            handle_pakets_receiver.is_finished(),
            "The handle of the pakets-receiver did not finish yet, even though 0.1 seconds passed."
        );

        assert!(
            handle_pakets_receiver.await.is_ok(),
            "The pakets-receiver panicked."
        );
        assert!(
            !handle_pakets_extractor.is_finished(),
            "The paket extractor returned before its cancellation token was erased."
        );

        cancellation_token_control.cancel();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        assert!(handle_pakets_extractor.is_finished(), "The pakets_extractor did not finish, even though its cancellation token was erased 0.1 second ago.");

        task_tracker.close();
        task_tracker.wait().await;
    }

    #[tokio::test]
    async fn test_stdin2tcp() {
        let num_messages_prompt_storm = 100;

        let prompt_storm = (1..=num_messages_prompt_storm).map(|num| -> Vec<char> {
            let prefix_len = "MESSAGE_n__[[[]]]\n".chars().count() + 1_usize + (num as f64).log10().floor() as usize;
            let prompt = format!{"MESSAGE_n_{}_[[[{}]]]\n", num, craft_random_text_of_len(Message::MAX_CONTENT_LEN - prefix_len)};
            prompt.chars().collect()
        }).flatten().collect::<String>();

        let prompts = format!("helo\n{prompt_storm}");

        let num_messages = prompts.chars().filter(|&char| char == '\n').count();

        let stdin = tokio_test::io::Builder::new()
            .read(prompts.as_bytes())
            .build();

        let stdin = tokio::io::BufReader::new(stdin);

        let (tx_pakets_extractor, mut rx_pakets_extractor) =
            tokio::sync::mpsc::channel::<Vec<u8>>(20);

        let cancellation_token_pakets_extractor = CancellationToken::new();
        let cancellation_token_pakets_extractor_control =
            cancellation_token_pakets_extractor.clone();
        let cancellation_token_pakets_extractor_check = cancellation_token_pakets_extractor.clone();

        let cancellation_token_stdin2tcp = CancellationToken::new();
        let cancellation_token_stdin2tcp_control = cancellation_token_stdin2tcp.clone();
        let cancellation_token_stdin2tcp_check = cancellation_token_stdin2tcp.clone();

        let test_task_tracker = TaskTracker::new();

        // server
        let handle_server = test_task_tracker.spawn(async move {
            let listener = tokio::net::TcpListener::bind(constant::SERVER_ADDR)
                .await
                .expect("The tcp listener did not bind.");

            let (socket_stream_server, _addr) = listener
                .accept()
                .await
                .expect("The listener could not accept the incoming connection request.");

            assert!(
                pakets_extractor(
                    socket_stream_server,
                    tx_pakets_extractor,
                    cancellation_token_pakets_extractor,
                )
                .await
                .is_ok(),
                "The paket extractor returned an error."
            );

            assert!(
                cancellation_token_pakets_extractor_check.is_cancelled(),
                "The paket extractor returned even though its cancellation token is still intact."
            );
        });

        // client
        let handle_client = test_task_tracker.spawn(async move {
            let socket_stream_client = TcpStream::connect(constant::SERVER_ADDR)
                .await
                .expect("The client could not connect to the server.");

            assert!(stdin2tcp(
                stdin,
                socket_stream_client,
                &Username::new("peppino").unwrap(),
                cancellation_token_stdin2tcp,
            ).await.is_ok(), "The stdin2tcp function returned an error.");

            assert!(cancellation_token_stdin2tcp_check.is_cancelled(), "The stdin2tcp function returned even though its cancellation token is still intact.");
        });

        let cancellation_token_message_receiver = CancellationToken::new();
        let cancellation_token_message_receiver_control =
            cancellation_token_message_receiver.clone();

        // message_receiver
        let handle_message_receiver = test_task_tracker.spawn(async move {
            let mut msg_count = 0;

            loop {
                if let Some(paket) = rx_pakets_extractor.recv().await {
                    let msg_result = Message::from_paket(paket);
                    assert!(msg_result.is_ok(), "The paket could not be converted successfully to a message.");
                    if let Ok(_msg) = msg_result {
                        msg_count += 1;
                        // println!("{}:> {}", _msg.get_username(), _msg.get_content());
                    }

                    // if msg_count%5 == 0 { println!("The message receiver processed successfully 5 more pakets. The message count is {msg_count}."); }

                    if msg_count == num_messages && cancellation_token_message_receiver.is_cancelled() {
                        break
                    }
                }
            }

            assert_eq!(msg_count, num_messages, "The number of messages sent by the server is bigger than the number of messages received by the client.");
        });

        assert!(
            !handle_server.is_finished(),
            "The server returned before the message receiver"
        );
        assert!(
            !handle_client.is_finished(),
            "The client returned before the message receiver"
        );

        cancellation_token_message_receiver_control.cancel();
        assert!(
            handle_message_receiver.await.is_ok(),
            "The message receiver returned an error."
        );

        cancellation_token_stdin2tcp_control.cancel();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(handle_client.is_finished(), "The handle_client did not finish, even though its cancellation token was erased 0.05 seconds ago.");

        assert!(handle_client.await.is_ok(), "The client returned an error.");

        cancellation_token_pakets_extractor_control.cancel();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(handle_server.is_finished(), "The server did not finish, even though its cancellation token was erased 0.05 seconds ago.");

        test_task_tracker.close();
        test_task_tracker.wait().await;
    }
}
