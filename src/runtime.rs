use std::future::Future;
use std::io;
use std::ops::Deref;
use std::sync::Arc;

pub trait Runtime {
    type Future<T>: Future<Output = io::Result<T>> + Send + 'static
    where
        T: Send + 'static;
    fn spawn_blocking<F, T>(&self, f: F) -> Self::Future<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static;
}

impl<P> Runtime for &P
where
    P: Runtime,
{
    type Future<T>
        = P::Future<T>
    where
        T: Send + 'static;
    fn spawn_blocking<F, T>(&self, f: F) -> Self::Future<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        (*self).spawn_blocking(f)
    }
}

impl<P> Runtime for Box<P>
where
    P: Runtime,
{
    type Future<T>
        = P::Future<T>
    where
        T: Send + 'static;
    fn spawn_blocking<F, T>(&self, f: F) -> Self::Future<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.deref().spawn_blocking(f)
    }
}

impl<P> Runtime for Arc<P>
where
    P: Runtime,
{
    type Future<T>
        = P::Future<T>
    where
        T: Send + 'static;
    fn spawn_blocking<F, T>(&self, f: F) -> Self::Future<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.deref().spawn_blocking(f)
    }
}

#[cfg(feature = "tokio-rt")]
const _: () = {
    use futures::TryFutureExt;

    type Future<T> = futures::future::MapErr<
        tokio::task::JoinHandle<T>,
        fn(tokio::task::JoinError) -> io::Error,
    >;
    impl Runtime for tokio::runtime::Runtime {
        type Future<T>
            = Future<T>
        where
            T: Send + 'static;

        fn spawn_blocking<F, T>(&self, f: F) -> Self::Future<T>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            self.spawn_blocking(f)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
        }
    }

    impl Runtime for tokio::runtime::Handle {
        type Future<T>
            = Future<T>
        where
            T: Send + 'static;

        fn spawn_blocking<F, T>(&self, f: F) -> Self::Future<T>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            self.spawn_blocking(f)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
        }
    }
};
