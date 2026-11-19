use kaban_chat::*;
use tokio::{self, net::TcpListener, task::JoinSet};

#[tokio::main]
async fn main() {
    println!(
        "Hello! You are the admin of the KabanChat server. It receives a maximum of {} wild boars at the address {}",
        konst::MAX_NUM_USERS,
        konst::SERVER_ADDRESS
    );

    server_manager(konst::SERVER_ADDRESS).await;
}

/// This function creates a socket and accepts connections on it, spawning for each of them a new
/// task handling it.
async fn server_manager(server_address: &str) {
    let listener = TcpListener::bind(server_address).await.unwrap();

    let joinset_server_manager: JoinSet<()> = JoinSet::new();

    loop {
        if joinset_server_manager.len() > konst::MAX_NUM_USERS {
            eprintln!(
                "A new connection was requested but the number of connected clients has reached the maximum."
            );
            break;
        }

        let (_tcp_stream, addr) = match listener.accept().await {
            Ok(connection_data) => connection_data,
            Err(err) => {
                eprintln!(
                    "########## There was an error in the connection with a client: {}",
                    err.to_string()
                );
                continue;
            }
        };

        println!(
            "A new connection has been established at the address '{}'",
            addr
        );
    }
}
