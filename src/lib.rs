mod async_functions;
mod information_units;
mod test_util;

// 1. first create the module, with a way for each id such that when it gets dropped
// it gets automatically given back to the server (how to do it?)
// 2. integrate this into the async_functions of the server, BUT NOT OF THE LIBRARY (not necessary to do in the client, hopefully)
mod id_pool;

pub mod prelude {
    pub use crate::async_functions::*;
    pub use crate::id_pool::*;
    pub use crate::information_units::*;
    pub use crate::test_util::*;
}
