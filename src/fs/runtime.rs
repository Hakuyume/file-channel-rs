use futures::TryFutureExt;
use std::io;

pub(super) type SpawnBlocking<T> =
    futures::future::MapErr<tokio::task::JoinHandle<T>, fn(tokio::task::JoinError) -> io::Error>;

pub(super) fn spawn_blocking<F, T>(f: F) -> SpawnBlocking<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}
