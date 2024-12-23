use std::future::Future;
use std::io;

pub trait SpawnBlocking {
    type Future<T>: Future<Output = io::Result<T>> + Send + 'static
    where
        T: Send + 'static;
    fn spawn_blocking<F, T>(&mut self, f: F) -> Self::Future<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static;
}

#[cfg(feature = "tokio-rt")]
#[derive(Clone, Debug)]
pub struct Tokio(pub tokio::runtime::Handle);
#[cfg(feature = "tokio-rt")]
const _: () = {
    use futures::TryFutureExt;

    impl SpawnBlocking for Tokio {
        type Future<T>
            = futures::future::MapErr<
            tokio::task::JoinHandle<T>,
            fn(tokio::task::JoinError) -> io::Error,
        >
        where
            T: Send + 'static;

        fn spawn_blocking<F, T>(&mut self, f: F) -> Self::Future<T>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            self.0
                .spawn_blocking(f)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
        }
    }

    impl Tokio {
        pub fn current() -> Self {
            Self(tokio::runtime::Handle::current())
        }
    }
};
