use futures::future::MapErr;
use futures::TryFutureExt;
use std::io;
use tokio::task::{JoinError, JoinHandle};

pub(crate) type SpawnBlocking<T> = MapErr<JoinHandle<T>, fn(JoinError) -> io::Error>;
pub(crate) fn spawn_blocking<F, T>(f: F) -> SpawnBlocking<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}
