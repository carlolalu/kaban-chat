use kaban_chat::*;

use std::io::Write;

use serde_json;

use tokio::{
    self,
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpStream,
    sync,
};
use tokio_stream::StreamExt;

use tokio_util::task::TaskTracker;

// ############################## MAIN ##############################
#[tokio::main]
async fn main() {
    println!("========================");
    println!("Welcome to the KabanChat!");
    println!(
        "WARNING: this chat is still in its alfa version, it is therefore absolutely unsecure, meaning that all that you write will be potentially readable by third parties."
    );
    println!("========================");
    println!();

    let (tcp_rd, tcp_wr, name) = connect_and_login().await;

    let main_tracker = TaskTracker::new();

    main_tracker.spawn(async move {
        wr_manager(tcp_wr, tokio::io::stdin(), &name).await;
    });

    main_tracker.spawn(async move {
        rd_manager(tcp_rd).await;
    });

    main_tracker.close();
    main_tracker.wait().await;
}

// ############################## FUNCTIONS ##############################
/// Asks for the nickname of the user, checks its validity, connects to the server, and greets him
/// ("helo" message). It returns the split Reader and Writer together with the username assigned.
async fn connect_and_login() -> (ReadHalf<TcpStream>, WriteHalf<TcpStream>, String) {
    let name = loop {
        print!(r###"Please input your desired userid and press "Enter": "###);
        std::io::stdout().flush().unwrap();

        let mut name = String::new();
        std::io::stdin().read_line(&mut name).unwrap();
        let name = name.trim().to_string();

        if name.chars().count() > Message::MAX_USERNAME_LEN || Message::has_invalid_chars(&name) {
            println!(
                r###"This username is too long or contains invalid chars! It must not be longer than {} chars!"###,
                Message::MAX_USERNAME_LEN
            );
        } else {
            break name;
        }
    };

    let stream = TcpStream::connect(constant::SERVER_ADDR).await.unwrap();
    let (tcp_rd, mut tcp_wr) = tokio::io::split(stream);

    let string_paket_of_helo_msg = Message::new(&name, "helo").unwrap().string_paket().unwrap();

    tcp_wr
        .write_all(string_paket_of_helo_msg.as_bytes())
        .await
        .unwrap();
    tcp_wr.flush().await.unwrap();

    (tcp_rd, tcp_wr, name.to_string())
}

/// Writes text into the prompt and packs it into messages. Then sends such messages to the server connection.
async fn wr_manager<Wr, Rd>(mut tcp_wr: Wr, mut stdin: Rd, name: &str) -> ()
where
    Wr: AsyncWrite + Unpin,
    Rd: AsyncRead + Unpin,
{
    // The choice of buffer_len as Message::MAX_CONTENT_LEN might be algorithmically practical but
    // maybe computationally less efficient, even though here it is not an issue.
    let mut prompt_buffer: Vec<u8> = Vec::with_capacity(Message::MAX_CONTENT_LEN * 10);

    'new_prompt: loop {
        print!("\n{name}:> ");
        std::io::stdout().flush().unwrap();

        '_process_prompt: loop {
            prompt_buffer.clear();
            match stdin.read_buf(&mut prompt_buffer).await {
                Ok(n) if n > 0 => {
                    let is_last_chunk = if prompt_buffer.last() == Some(&b'\n') {
                        prompt_buffer.pop();
                        true
                    } else {
                        false
                    };

                    if prompt_buffer.len() > 0 {
                        let text = String::from_utf8(prompt_buffer.clone()).unwrap();

                        let msg_results: Vec<_> = Message::many_try_new(&name, &text);
                        let mut msg_result_stream = tokio_stream::iter(msg_results);

                        while let Some(msg_result) = msg_result_stream.next().await {
                            tcp_wr.write_all(msg_result.unwrap().string_paket().unwrap().as_bytes()).await.unwrap();
                        }
                    }

                    if is_last_chunk {
                        continue 'new_prompt;
                    }
                }
                Ok(_) | _ => panic!(),
            }
        }
    }
}

/// Receives messages from the server and prints them in the stdin
async fn rd_manager(tcp_rd: impl AsyncRead + Unpin + Send + 'static) {
    let (msg_tx, mut msg_rx) = sync::mpsc::channel(10);

    let tcp_rd_tracker = TaskTracker::new();

    tcp_rd_tracker.spawn(async move {
        handle_msgs_reader(tcp_rd, msg_tx).await.unwrap();
    });

    tcp_rd_tracker.spawn(async move {
        let msg_result = msg_rx.recv().await.unwrap();
        let msg = msg_result.unwrap();
        println!("{}:> {}", msg.get_username(), msg.get_content());
    });

    tcp_rd_tracker.close();
    tcp_rd_tracker.wait().await;
}
