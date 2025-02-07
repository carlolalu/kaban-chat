use std::net::Shutdown;
use kaban_chat::prelude::*;

use thiserror::Error;

use tokio::{
    self,
    io::{AsyncRead, AsyncWriteExt},
    net::TcpStream,
    sync,
    signal,
};
use tokio_util::sync::CancellationToken;

use tokio_util::task::TaskTracker;

// ############################## MAIN ##############################
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("========================");
    println!("Welcome to the KabanChat!");
    println!(
        "WARNING: this chat is still in its alfa version, it is therefore absolutely unsecure! Do not share private information!"
    );
    println!("========================");
    println!();

    let username = Username::choose();

    let stream = TcpStream::connect(constant::SERVER_ADDR).await?;
    let (tcp_rd, mut tcp_wr) = tokio::io::split(stream);

    tcp_wr.write_all(&Message::craft_msg_helo(&username).paket()).await.expect("The helo message could not be sent");

    let main_tracker = TaskTracker::new();

    let shutdown_token = CancellationToken::new();
    let shutdown_token_wr = shutdown_token.clone();
    let shutdown_token_rd = shutdown_token.clone();


    main_tracker.spawn(async move {
        stdin2tcp(tokio::io::stdin(), tcp_wr, &username, shutdown_token_wr)
            .await;
    });

    main_tracker.spawn(async move {
        rd_manager(tcp_rd, shutdown_token_rd).await;
    });

    main_tracker.close();
    main_tracker.wait().await;


    // println!("The shutdown process is completed. It was nice to let you chat, see you soon!");

    Ok(())
}

// ############################## FUNCTIONS ##############################

async fn shutdown_watch_dog(shutdown_token: CancellationToken) {
    let shutdown_token_recv = shutdown_token.clone();

    tokio::select! {
        _ctrlc = signal::ctrl_c() => {},
        _token = shutdown_token_recv.cancelled() => {},
    }

    shutdown_token.cancel();

    //println!();
    //println!("========================");
    //println!("Starting the shutdown process");
}

/// Receives messages from the server and prints them in the stdin
async fn rd_manager(tcp_rd: impl AsyncRead + Unpin + Send + 'static, shutdown_token: CancellationToken) -> Result<(), RdManagerError> {
    // todo: no magic numbers
    let (paket_tx, mut paket_rx) = sync::mpsc::channel(10);

    let tcp_rd_tracker = TaskTracker::new();

    let shutdown_token_pakets_extractor = shutdown_token.clone();
    let shutdown_token_pakets_receiver = shutdown_token.clone();

    tcp_rd_tracker.spawn(async move {
        pakets_extractor(tcp_rd, paket_tx, shutdown_token_pakets_extractor).await;
    });

    tcp_rd_tracker.spawn(async move {
        'receiving_pakets: loop {
            tokio::select! {
                _shutdown = shutdown_token_pakets_receiver.cancelled() => {
                    break 'receiving_pakets;
                }
                maybe_paket = paket_rx.recv() => {
                    if let Some(paket) = maybe_paket {
                                        let msg = Message::from_paket(paket).unwrap();
                println!("{}:> {}", msg.get_username(), msg.get_content());
                    }
                }
            }
        }

        // todo: here return eventually errors
    });

    shutdown_token.cancel();

    tcp_rd_tracker.close();
    tcp_rd_tracker.wait().await;

    Ok(())
}

#[derive(Error, Debug, PartialEq)]
#[error(transparent)]
enum RdManagerError {
    // todo
    #[error(
        "No paket init byte delimiter () found in the utf8 sample. The sample is: [[]]."
    )]
    Error,
}
