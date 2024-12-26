mod receiver;
mod runtime;
mod sender;
mod state;

pub use receiver::Receiver;
pub use sender::Sender;
use std::fs::File;
use std::sync::Arc;
use std::sync::Mutex;

const DEFAULT_BUF_SIZE: usize = 1 << 16;

pub fn channel(file: File) -> (Sender, Receiver) {
    let file = Arc::new(file);
    let state = Arc::new(Mutex::default());
    (
        Sender::new(DEFAULT_BUF_SIZE, file.clone(), state.clone()),
        Receiver::new(DEFAULT_BUF_SIZE, file, state),
    )
}

#[cfg(test)]
mod tests;
