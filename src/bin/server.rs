// #################### IMPORTS ####################
use kaban_chat::*;

use std::sync::Arc;

use tokio::{self, net::TcpStream, sync};

use tokio_util::task::TaskTracker;

// #################### MAIN ####################

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

// #################### FUNCTIONS ####################

/// This function is responsible for passing dispatches between client_handlers
async fn dispatcher(
    mut dispatcher_rx: sync::mpsc::Receiver<Dispatch>,
    dispatcher_mic: Arc<sync::broadcast::Sender<Dispatch>>,
) -> () {
    loop {
        if let Some(dispatch) = dispatcher_rx.recv().await {
            dispatcher_mic.send(dispatch).unwrap();
        }
    }
}

/// This function creates a socket and accepts connections on it, spawning for each of them a new task handling it.
async fn server_manager(
    dispatcher_tx_arc: Arc<sync::broadcast::Sender<Dispatch>>,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
) {
}

/// The client handler divides the stream into reader and writer, and then spawns two tasks handling them.
async fn client_handler(
    stream: TcpStream,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    addr: std::net::SocketAddr,
) {
}
