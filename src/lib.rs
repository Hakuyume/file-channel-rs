mod receiver;
mod runtime;
mod sender;
mod state;

pub use receiver::Receiver;
pub use runtime::Runtime;
pub use sender::Sender;
use std::fs::File;
use std::sync::Arc;
use std::sync::Mutex;

// https://github.com/rust-lang/rust/blob/426d1734238e3c5f52e935ba4f617f3a9f43b59d/library/std/src/sys_common/io.rs#L3
const DEFAULT_BUF_SIZE: usize = 8 * 1024;

pub fn channel<R>(runtime: R, file: File) -> (Sender<R>, Receiver<R>)
where
    R: Clone + Runtime,
{
    let file = Arc::new(file);
    let state = Arc::new(Mutex::default());
    (
        Sender::new(
            runtime.clone(),
            DEFAULT_BUF_SIZE,
            file.clone(),
            state.clone(),
        ),
        Receiver::new(runtime, DEFAULT_BUF_SIZE, file, state),
    )
}

#[cfg(test)]
mod tests;
