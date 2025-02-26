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

            // todo: are we sure we just wanna drop the whole function wihtout waiting for all
            // pakets to arrive, i.e. for the tcp_reader to not return bytes anymore? Is it possible to do it, or would the call remain eternally suspended?
            _cancellation = cancellation_token.cancelled() => break 'writing_on_buffer Ok(()),
        };

        match result_tcp_read {
            Ok(n) if n > 0 => {
                // Loop assumption: previous_fragment is initialised (eventually empty)

                // todo: write the algorithm here and depending on what you return break the loop or not. document this function very well.
                process_pakets_buffer(&mut previous_fragment, &mut buffer, &mut tcp_reader, &tx).await;
                // TODO: make something depending on the error returned by the function above
                // If I return Ok then I should just continue, otherwise depending on the error I should do smt similar
            }
            Ok(_zero) => {
                let err = Err(PaketsExtractorError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "The tcp reader read 0 bytes: the connection was interrupted.",
                )));
                eprint_small_error(err);
                break 'writing_on_buffer Ok(());
            }
            Err(err) => break 'writing_on_buffer Err(PaketsExtractorError::Io(err)),
        }
    };

    cancellation_token.cancel();

    outcome
}

async fn process_pakets_buffer(previous_fragment: &mut Vec<u8>, buffer: &mut Vec<u8>, tcp_reader: &mut (impl AsyncRead + Unpin + Send + 'static), tx: &sync::mpsc::Sender<Vec<u8>>) -> Result<(), // which type of error do I want
> {
    let actual_fragments: Vec<&[u8]> = buffer
        .split_inclusive(|&byte| byte == Message::PAKET_END_U8)
        .collect();

    let (first_fragment, subsequent_fragments) = match actual_fragments.split_first() {
        Some(smt) => (*smt.0, smt.1),
        None => {
            eprint_small_error(UnusualEmptyEntity {
                entity: "actual_fragments".to_string(),
            });
            return Ok(());
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
            None => return Ok(()),
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
                eprint_small_error(UnusualEmptyEntity {
                    entity: "last_fragment".to_string(),
                });
                return Ok(());
            }
        }
    } else {
        previous_fragment.append(&mut first_fragment.to_vec());

        if previous_fragment.len() > Message::MAX_PAKET_U8_LEN {
            let err = PaketsExtractorError::TooLongPaket {
                paket_u8_len: previous_fragment.len(),
            };
            eprint_small_error(err);

            let reinitialization_outcome = reinitialize_previous_fragment(
                previous_fragment,
                tcp_reader,
                buffer,
            ).await;

            if reinitialization_outcome.is_err() {
                return reinitialization_outcome;
            }
        }
    }
    Ok(())
}


// TODO: this algorithm is wrong: if you divide the buffer in two at first end symbol, what happens is that
// previous_fragment might get not just one paket, but many, breaking the assumption on the next cycle.
// This must be done otherwise and MORE ORDERLY.

/// If the pakets_extractor drops a paket due to some errors, it is important to reinitialise
/// 'previous_fragment'.
async fn reinitialize_previous_fragment(
    previous_fragment: &mut Vec<u8>,
    tcp_reader: &mut (impl AsyncRead + Unpin + Send + 'static),
    buffer: &mut Vec<u8>,
) -> Result<(), PaketsExtractorError> {
    previous_fragment.clear();
    'reinitialising_previous_fragment: loop {
        buffer.clear();
        match tcp_reader.read_buf(buffer).await {
            Ok(n) if n > 0 => {
                if buffer.contains(&Message::PAKET_END_U8) {
                    let fragments: Vec<&[u8]> = buffer
                        .splitn(2, |&byte| byte == Message::PAKET_END_U8)
                        .collect();
                    *previous_fragment = match fragments.get(1) {
                        Some(&slice) => slice.to_vec(),
                        None => {
                            let err = UnusualEmptyEntity {
                                entity: "slice in the loop <'reinitialising_previous_fragment>"
                                    .to_string(),
                            };
                            eprint_small_error(err);
                            continue 'reinitialising_previous_fragment;
                        }
                    }
                } else {
                    continue 'reinitialising_previous_fragment;
                }
            }
            Ok(_zero) => return Ok(()),
            Err(err) => return Err(PaketsExtractorError::Io(err)),
        }
    }
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
    use tokio_util::task::TaskTracker;

    #[tokio::test]
    async fn test_pakets_extractor() {
        let num_messages = 100;

        let paket: Vec<u8> = (0..num_messages)
            .map(|num| {
                let username =
                    Username::new(&format!("user{}", num)).expect("This username is not valid");
                let random_message = craft_random_msg(&username);
                random_message.paket()
            })
            .flatten()
            .collect();

        let reader = tokio_test::io::Builder::new().read(&paket).build();

        let (tx, mut rx) = tokio::sync::mpsc::channel(20);

        let task_tracker = TaskTracker::new();
        let mut handles = Vec::new();

        handles.push(task_tracker.spawn(async move {
            pakets_extractor(reader, tx)
                .await
                .expect("The message depaketer failed.");
        }));

        handles.push(task_tracker.spawn(async move {
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
            if handle.await.is_err() {
                panic!("One of the tasks panicked!")
            }
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

        let prompt_buffer1 = format!("{}\n", craft_random_text_of_len(Message::MAX_CONTENT_LEN));

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
