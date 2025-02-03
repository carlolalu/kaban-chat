use kaban_chat::*;

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::{self, io, net::TcpStream, sync};

use tokio_util::task::TaskTracker;

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
) -> () {
    loop {
        if let Some(dispatch) = dispatcher_rx.recv().await {
            dispatcher_mic.send(dispatch).unwrap();
        }
    }
}

/// This function creates a socket and accepts connections on it, spawning for each of them a new
/// task handling it.
async fn server_manager(
    dispatcher_tx_arc: Arc<sync::broadcast::Sender<Dispatch>>,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
) {
    let listener = TcpListener::bind(constant::SERVER_ADDR).await.unwrap();

    let mut next_user_id: usize = 1;

    let server_manager_tracker = TaskTracker::new();

    loop {
        if server_manager_tracker.len() > constant::MAX_NUM_USERS || next_user_id > usize::MAX {
            eprintln!("A new connection was requested but the number of connected clients has reached the maximum\
            or the userid reached its maximum.");
            break;
        }
        let (stream, addr) = listener.accept().await.unwrap();
        let client_handler_tx = client_handler_tx.clone();
        let dispatcher_subscriber = dispatcher_tx_arc.clone();

        let client_handler_rx = dispatcher_subscriber.subscribe();

        server_manager_tracker.spawn(async move {
            client_handler(
                stream,
                client_handler_tx,
                client_handler_rx,
                addr,
                next_user_id,
            )
            .await;
        });

        next_user_id = next_user_id + 1;
    }

    server_manager_tracker.close();
    server_manager_tracker.wait().await;
}

/// The client handler divides the stream into reader and writer, and then spawns two tasks handling them.
async fn client_handler(
    stream: TcpStream,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    _addr: std::net::SocketAddr,
    userid: usize,
) {
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
}

async fn client_tcp_wr_loop(
    mut tcp_wr: impl AsyncWrite + Unpin,
    mut client_handler_rx: sync::broadcast::Receiver<Dispatch>,
    userid: usize,
) {
    loop {
        let dispatch = client_handler_rx.recv().await.unwrap();

        if dispatch.get_userid() != userid {
            let serialized_msg = serde_json::to_string(&dispatch.into_msg()).unwrap();
            tcp_wr.write_all(serialized_msg.as_bytes()).await.unwrap();
        }
    }
}

async fn client_tcp_rd_loop(
    tcp_rd: impl AsyncRead + Unpin + Send + 'static,
    client_handler_tx: sync::mpsc::Sender<Dispatch>,
    userid: usize,
) {
    let (msg_tx, mut msg_rx) = sync::mpsc::channel(10);

    let tcp_rd_tracker = TaskTracker::new();

    tcp_rd_tracker.spawn(async move {
        handle_msgs_reader(tcp_rd, msg_tx).await;
    });

    tcp_rd_tracker.spawn(async move {
        let msg = msg_rx.recv().await.unwrap();
        let dispatch = Dispatch::new(userid, msg);
        client_handler_tx.send(dispatch).await.unwrap();
    });

    tcp_rd_tracker.close();
    tcp_rd_tracker.wait().await;
}
