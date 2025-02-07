use kaban_chat::*;

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::{self, io, net::TcpStream, sync};

use tokio_util::task::TaskTracker;

use thiserror::Error;

// ############################## MAIN ##############################

#[tokio::main]
async fn main() {
    println!(
        "Welcome, the KabanChat server receives at maximum {} users at the address {}",
        constant::MAX_NUM_USERS,
        constant::SERVER_ADDR
    );

    let (client_handler_tx, dispatcher_rx) = sync::mpsc::channel::<Dispatch>(50);
    let (dispatcher_tx, _rx) = sync::broadcast::channel::<Dispatch>(50);

    let dispatcher_tx_arc = Arc::new(dispatcher_tx);
    let dispatcher_mic = dispatcher_tx_arc.clone();

    let main_tracker = TaskTracker::new();

    main_tracker.spawn(async move {
        dispatcher(dispatcher_rx, dispatcher_mic).await;
    });

    main_tracker.spawn(async move {
        server_manager(dispatcher_tx_arc, client_handler_tx).await;
    });

    main_tracker.close();
    main_tracker.wait().await;

    println!("The server is shutting down!");
}

// ############################## FUNCTIONS ##############################

/// This function is responsible for passing dispatches between client_handlers
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

    let server_manager_tracker = TaskTracker::new();

    loop {
        if server_manager_tracker.len() > constant::MAX_NUM_USERS || next_user_id > usize::MAX {
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

        server_manager_tracker.spawn(async move {
            match client_handler(
                stream,
                client_handler_tx,
                client_handler_rx,
                addr,
                next_user_id,
            )
            .await
            {
                Ok(()) => (),
                Err(err) => {
                    eprint_small_error(err);
                }
            }
        });

        next_user_id = next_user_id + 1;
    }

    server_manager_tracker.close();
    server_manager_tracker.wait().await;

    Ok(())
}

/// The client handler divides the stream into reader and writer, and then spawns two tasks handling them.
async fn client_handler(
    stream: TcpStream,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    _addr: std::net::SocketAddr,
    userid: usize,
) -> Result<(), ClientHandlerError> {
    let client_handler_task_manager = TaskTracker::new();

    let (tcp_rd, tcp_wr) = io::split(stream);

    client_handler_task_manager.spawn(async move {
        client_tcp_wr_loop(tcp_wr, client_handler_rx, userid).await;
    });

    client_handler_task_manager.spawn(async move {
        client_tcp_rd_loop(tcp_rd, client_handler_tx, userid).await;
    });

    client_handler_task_manager.close();
    client_handler_task_manager.wait().await;

    Ok(())
}

#[derive(Error, Debug, PartialEq)]
enum ClientHandlerError {
    #[error("The 'client_tcp_wr_loop' died because of this error: ")]
    TrialError,
}

async fn client_tcp_wr_loop(
    mut tcp_wr: impl AsyncWrite + Unpin,
    mut client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    userid: usize,
) -> Result<(), tokio::io::Error> {
    loop {
        let dispatch = client_handler_rx
            .recv()
            .await
            .expect("The client wr loop could not receive the dispatch from the dipatcher");

        if dispatch.get_userid() != userid {
            tcp_wr.write_all(&dispatch.get_bytes()).await?;
        }
    }

    // todo: take away with graceful shutdown
    #[allow(unreachable_code)]
    Ok(())
}

async fn client_tcp_rd_loop(
    tcp_rd: impl AsyncRead + Unpin + Send + 'static,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    userid: usize,
) -> Result<(), tokio::io::Error> {
    // TODO no magic numbers
    let (pakets_tx, mut pakets_rx) = sync::mpsc::channel(300);

    let tcp_rd_tracker = TaskTracker::new();

    tcp_rd_tracker.spawn(async move {
        // todo: use this result
        let _result = pakets_extractor(tcp_rd, pakets_tx).await;
    });

    tcp_rd_tracker.spawn(async move {
        loop {
            if let Some(bytes) = pakets_rx.recv().await {
                let dispatch = Dispatch::new(userid, bytes);
                client_handler_tx
                    .send(dispatch)
                    .await
                    .expect("this error must still be properly defined");
            }
        }
    });

    tcp_rd_tracker.close();
    tcp_rd_tracker.wait().await;

    Ok(())
}
