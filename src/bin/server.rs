use kaban_chat::prelude::*;
use std::error::Error;

use std::sync::Arc;

use tokio::{
    io::{self, AsyncRead, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync,
};

use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

// ############################## MAIN ##############################

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    println!(
        "Welcome, the KabanChat server receives at maximum {} users at the address {}",
        constant::MAX_NUM_USERS,
        constant::SERVER_ADDR
    );

    let (client_handler_tx, dispatcher_rx) = sync::mpsc::channel::<Dispatch>(50);
    let (dispatcher_tx, _rx) = sync::broadcast::channel::<Dispatch>(50);

    let dispatcher_tx_arc = Arc::new(dispatcher_tx);
    let dispatcher_mic = dispatcher_tx_arc.clone();

    let tasktracker_main = TaskTracker::new();

    let handle_dispatcher = tasktracker_main.spawn(async move {
        dispatcher(dispatcher_rx, dispatcher_mic).await?;
        Ok::<(), sync::broadcast::error::SendError<Dispatch>>(())
    });

    let handle_server_manager = tasktracker_main.spawn(async move {
        server_manager(dispatcher_tx_arc, client_handler_tx).await?;
        Ok::<(), tokio::io::Error>(())
    });

    tokio::select! {
        join_result = handle_dispatcher => {
            join_result??
        },
        join_result = handle_server_manager => {
            join_result??
        }
    }

    tasktracker_main.close();
    tasktracker_main.wait().await;

    println!("The server is shutting down!");

    Ok(())
}

// ############################## FUNCTIONS ##############################

/// This function is responsible for passing dispatches between client_handlers.
async fn dispatcher(
    mut dispatcher_rx: sync::mpsc::Receiver<Dispatch>,
    dispatcher_mic: Arc<sync::broadcast::Sender<Dispatch>>,
) -> Result<(), sync::broadcast::error::SendError<Dispatch>> {
    loop {
        if let Some(dispatch) = dispatcher_rx.recv().await {
            // The broadcast returns an error if there are no subscribers, but the only case in
            // which a dispatch is received and there are no subscribers is this: graceful shutdown
            // is implemented, the server has no clients, and is shutting down thus sending the
            // dispatch "I am shutting down" to all clients. This specific case will be addressed
            // only when graceful shutdown will be implemented.
            // todo: when implementing graceful shutdown check here up.
            dispatcher_mic.send(dispatch)?;
        }
    }

    // remove when graceful shutdown
    #[allow(unreachable_code)]
    Ok(())
}

/// This function creates a socket and accepts connections on it, spawning for each of them a new
/// task handling it.
async fn server_manager(
    dispatcher_tx_arc: Arc<sync::broadcast::Sender<Dispatch>>,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
) -> Result<(), tokio::io::Error> {
    let listener = TcpListener::bind(constant::SERVER_ADDR).await?;

    let mut next_user_id: usize = 1;

    let mut joinset_server_manager = JoinSet::new();

    loop {
        if joinset_server_manager.len() > constant::MAX_NUM_USERS || next_user_id > usize::MAX {
            eprintln!("A new connection was requested but the number of connected clients has reached the maximum\
            or the userid reached its maximum.");
            break;
        }
        let (stream, addr) = match listener.accept().await {
            Ok(connection_data) => connection_data,
            Err(err) => {
                eprint_small_error(err);
                continue;
            }
        };

        let client_handler_tx = client_handler_tx.clone();
        let dispatcher_subscriber = dispatcher_tx_arc.clone();

        let client_handler_rx = dispatcher_subscriber.subscribe();

        joinset_server_manager.spawn(async move {
            client_handler(
                stream,
                client_handler_tx,
                client_handler_rx,
                addr,
                next_user_id,
            )
            .await
            .unwrap_or_else(|err| eprint_small_error(err));
            Ok::<(), ClientHandlerError>(())
        });

        next_user_id = next_user_id + 1;
    }

    Ok(())
}

/// The client handler divides the stream into reader and writer, and then spawns two tasks handling them.
async fn client_handler(
    tcp_stream: impl AsyncWrite + AsyncRead + std::marker::Send + 'static,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    address: std::net::SocketAddr,
    userid: usize,
) -> Result<(), ClientHandlerError> {
    let tasktracker_client_handler = TaskTracker::new();

    let (tcp_rd, tcp_wr) = io::split(tcp_stream);

    let client_cancellation = CancellationToken::new();
    let reader_cancellation_controller = client_cancellation.clone();
    let writer_cancellation_controller = client_cancellation.clone();

    let handle_writer = tasktracker_client_handler.spawn(async move {
        client_wr_process(
            tcp_wr,
            client_handler_rx,
            userid,
            writer_cancellation_controller,
        )
        .await?;
        Ok::<(), ClientHandlerErrorCause>(())
    });

    let handle_reader = tasktracker_client_handler.spawn(async move {
        client_rd_process(
            tcp_rd,
            client_handler_tx,
            userid,
            reader_cancellation_controller,
        )
        .await?;
        Ok::<(), ClientHandlerErrorCause>(())
    });

    let first_finished = tokio::select! {
        join_result = handle_reader => {
            join_result
        },
        join_result = handle_writer => {
            join_result
        },
        // here add another arm for the general_graceful_shutdown_token, when the time comes
        // this one should call the client_cancellation_token.cancel()
    };

    client_cancellation.cancel();

    let final_result = match first_finished {
        Ok(join_output) => match join_output {
            Ok(_) => Ok(()),
            Err(err) => Err(ClientHandlerError {
                id: userid,
                address,
                cause: err,
            }),
        },
        Err(join_error) => Err(ClientHandlerError {
            id: userid,
            address,
            cause: ClientHandlerErrorCause::Join(join_error),
        }),
    };

    final_result
}

#[derive(Error, Debug)]
#[error("The client_handler with id {id} handling the address {address:?} failed. The cause was: {cause}")]
struct ClientHandlerError {
    id: usize,
    address: std::net::SocketAddr,
    cause: ClientHandlerErrorCause,
}

#[derive(Error, Debug)]
#[error(transparent)]
enum ClientHandlerErrorCause {
    ClientWriter(#[from] ClientWriterError),
    ClientReader(#[from] ClientReaderError),
    Join(#[from] tokio::task::JoinError),
}

async fn client_wr_process(
    mut tcp_wr: impl AsyncWrite + Unpin,
    mut client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    userid: usize,
    cancellation_token: CancellationToken,
) -> Result<(), ClientWriterError> {
    'writing_pakets_into_tcp: loop {
        let dispatch = tokio::select! {
            maybe_dispatch = client_handler_rx.recv() => {
                maybe_dispatch?
            },
            _cancellation = cancellation_token.cancelled() => {
                break 'writing_pakets_into_tcp;
            }
        };

        if dispatch.get_userid() != userid {
            tcp_wr.write_all(&dispatch.get_bytes()).await?;
        }
    }

    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
enum ClientWriterError {
    Io(#[from] tokio::io::Error),
    RecvFromDispatcher(#[from] tokio::sync::broadcast::error::RecvError),
}

async fn client_rd_process(
    tcp_rd: impl AsyncRead + Unpin + Send + 'static,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    userid: usize,
    cancellation_token: CancellationToken,
) -> Result<(), ClientReaderError> {
    let pakets_buffer_len = 30;
    let (pakets_tx, mut pakets_rx) = sync::mpsc::channel(pakets_buffer_len);

    let tasktracker_client_rd_handler = TaskTracker::new();

    let paket_extractor_cancellation = cancellation_token.clone();
    let cancellation_sender = cancellation_token.clone();

    let handle_pakets_extractor = tasktracker_client_rd_handler.spawn(async move {
        pakets_extractor(tcp_rd, pakets_tx, paket_extractor_cancellation).await?;
        Ok::<(), ClientReaderError>(())
    });

    let handle_sender = tasktracker_client_rd_handler.spawn(async move {
        'reading_messages: loop {
            let maybe_bytes = tokio::select! {
                result_receive_pakets_from_tcp_reader = pakets_rx.recv() => {
                    result_receive_pakets_from_tcp_reader
                },
                _ = cancellation_sender.cancelled() => {
                    break 'reading_messages;
                },
            };

            if let Some(bytes) = maybe_bytes {
                let dispatch = Dispatch::new(userid, bytes);
                client_handler_tx.send(dispatch).await?;
            }
        }

        Ok::<(), ClientReaderError>(())
    });

    tokio::select! {
        join_result = handle_pakets_extractor => {
            join_result??
        },
        join_result = handle_sender => {
            join_result??
        },
    }

    cancellation_token.cancel();

    tasktracker_client_rd_handler.close();
    tasktracker_client_rd_handler.wait().await;

    Ok::<(), ClientReaderError>(())
}

#[derive(Error, Debug)]
#[error(transparent)]
enum ClientReaderError {
    SendToDispatcher(#[from] tokio::sync::mpsc::error::SendError<Dispatch>),
    PaketsExtractor(#[from] PaketsExtractorError),
    Join(#[from] tokio::task::JoinError),
}
