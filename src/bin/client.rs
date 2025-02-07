use kaban_chat::*;

use std::io::Write;
use thiserror::Error;

use tokio::{
    self,
    io::{AsyncRead, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpStream,
    sync,
};

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

    tcp_wr.write_all(&Message::helo_msg(&username).paket()).await.expect("The helo message could not be sent");

    let main_tracker = TaskTracker::new();

    main_tracker.spawn(async move {
        stdin2tcp(tokio::io::stdin(), tcp_wr, &username)
            .await;
    });

    main_tracker.spawn(async move {
        rd_manager(tcp_rd).await;
    });

    main_tracker.close();
    main_tracker.wait().await;

    Ok(())
}

// ############################## FUNCTIONS ##############################

/// Receives messages from the server and prints them in the stdin
async fn rd_manager(tcp_rd: impl AsyncRead + Unpin + Send + 'static) -> Result<(), RdManagerError> {
    // todo: no magic numbers
    let (paket_tx, mut paket_rx) = sync::mpsc::channel(10);

    let tcp_rd_tracker = TaskTracker::new();

    tcp_rd_tracker.spawn(async move {
        pakets_extractor(tcp_rd, paket_tx).await;
    });

    tcp_rd_tracker.spawn(async move {
        loop {
            if let Some(paket) = paket_rx.recv().await {
                let msg = Message::from_paket(paket).unwrap();
                println!("{}:> {}", msg.get_username(), msg.get_content());
            }
        }
    });

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
