# kaban-chat
A chat made in Rust to practice my skills


## Developer's Guide

The server and the clients are exchanging Message(s), i.e. structs `struct Message { username: String, content: String, }`. The server has a structure of this kind:

```mermaid
flowchart TB
    wr_manager1 --"sends Messages to"-->client_tcp_rd_loop1
    client_tcp_wr_loop1 --"sends Messages to"--> rd_manager1

    wr_manager2 -- "sends Messages to" -->client_tcp_rd_loop2
    client_tcp_wr_loop2 --"sends Messages to"--> rd_manager2

    CLIENT1 --"connects to"--> SERVER_MANAGER

    CLIENT2 --"connects to"--> SERVER_MANAGER
    
    subgraph CLIENT1 ["client 1"]
        direction LR
        stdin1-->wr_manager1
        rd_manager1-->stdout1
    end
    
    subgraph CLIENT2 ["client 2"]
        direction LR
        stdin2-->wr_manager2
        rd_manager2-->stdout2
    end
        
    subgraph SERVER
        client_handler1 --> client_tcp_rd_loop1 --"send dispatches to"--> 
            DISPATCHER --"forward dispatches to"--> client_tcp_wr_loop1
        client_handler1 --> client_tcp_wr_loop1
        SERVER_MANAGER --"delegates"--> client_handler1

        client_handler2 --> client_tcp_rd_loop2 --"send dispatches to"-->
        DISPATCHER --"forward dispatches to"--> client_tcp_wr_loop2
        client_handler2 --> client_tcp_wr_loop2
        SERVER_MANAGER --"delegates"--> client_handler2
    end
```

### Error types

The choice had to be made between generic errors which could easily be converted (on the trace of the `anyhow` crate) or a bit more precise errors (on the trace of the `thiserror` crate). The choice fell on the latter one.

- `lib.rs`:
  - `enum MessageError`
    - TooLongUsername
    - TooLongContent
    - InvalidCharsUsername
    - InvalidCharsContent
  - `enum ReaderError`
    - conversion from async io error
    - propagation of MessageError


## Notes for me

- test: you should write quite a lot of unit tests
- implement error_handling: write your own errors or convert with anyhow crate? (search for `panic` and `unwrap`)

- graceful shutdown: add the id pool and recall to check for the id_pool token redemption
- id pool: it could be sharded to avoid bottlenecks if many client_handlers must access the same resource.