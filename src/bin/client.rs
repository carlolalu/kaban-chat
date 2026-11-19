use kaban_chat::*;
use tokio::{self, net::TcpStream};

#[tokio::main]
async fn main() {
    println!(
        "Hello KabanChat user! You will now connect with the KabanChat server at the address {}",
        konst::SERVER_ADDRESS
    );

    connect(konst::SERVER_ADDRESS).await;
}

/// Asks for the nickname of the user, checks its validity, connects to the server, and greets him
/// ("helo" message). It returns the split Reader and Writer together with the username assigned.
async fn connect(server_addr: &str) {
    let stream = TcpStream::connect(server_addr).await.unwrap();

    println!(
        "You local address is {}",
        stream.local_addr().unwrap().to_string()
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;
}
