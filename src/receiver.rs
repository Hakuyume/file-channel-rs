use crate::runtime::Runtime;
use futures::future::{Fuse, FusedFuture};
use futures::{FutureExt, Stream};
use std::fs::File;
use std::future::Future;
use std::io;
use std::os::unix::fs::FileExt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

#[pin_project::pin_project(!Unpin)]
#[derive(Clone)]
pub struct Receiver<R>(#[pin] Inner<R>, Guard)
where
    R: Runtime;

impl<R> Receiver<R>
where
    R: Runtime,
{
    pub(crate) fn new(
        runtime: R,
        capacity: usize,
        file: Arc<File>,
        state: Arc<Mutex<crate::state::State>>,
    ) -> Self {
        Self(
            Inner::new(runtime, capacity, file),
            Guard { state, key: None },
        )
    }
}

impl<R> Stream for Receiver<R>
where
    R: Runtime,
{
    type Item = io::Result<Vec<u8>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            let mut state = this.1.state.lock().unwrap();
            if state.offset <= this.0.offset() {
                if state.is_closed {
                    break Poll::Ready(None);
                } else {
                    let key = this.1.key.get_or_insert_with(|| state.wakers.insert(None));
                    state.wakers[*key] = Some(cx.waker().clone());
                    break Poll::Pending;
                }
            }
            let item = ready!(this.0.as_mut().poll(cx))?;
            if !item.is_empty() {
                break Poll::Ready(Some(Ok(item)));
            }
        }
    }
}

struct Guard {
    state: Arc<Mutex<crate::state::State>>,
    key: Option<usize>,
}

impl Clone for Guard {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            key: None,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(key) = self.key {
            if let Ok(mut state) = self.state.lock() {
                state.wakers.remove(key);
            }
        }
    }
}

#[pin_project::pin_project]
struct Inner<R>
where
    R: Runtime,
{
    runtime: R,
    capacity: usize,
    file: Arc<File>,
    offset: u64,
    #[pin]
    f: Fuse<R::Future<io::Result<Vec<u8>>>>,
}

impl<R> Clone for Inner<R>
where
    R: Clone + Runtime,
{
    fn clone(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
            capacity: self.capacity,
            file: self.file.clone(),
            offset: self.offset,
            f: Fuse::terminated(),
        }
    }
}

impl<R> Inner<R>
where
    R: Runtime,
{
    fn new(runtime: R, capacity: usize, file: Arc<File>) -> Self {
        Self {
            runtime,
            capacity,
            file,
            offset: 0,
            f: Fuse::terminated(),
        }
    }

    fn offset(&self) -> u64 {
        self.offset
    }

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Vec<u8>>> {
        let mut this = self.project();

        if this.f.is_terminated() {
            let capacity = *this.capacity;
            let file = this.file.clone();
            let offset = *this.offset;
            let f = this.runtime.spawn_blocking(move || {
                let mut buf = Vec::with_capacity(capacity);
                unsafe { buf.set_len(buf.capacity()) };
                let n = file.read_at(&mut buf, offset)?;
                buf.truncate(n);
                Ok(buf)
            });
            this.f.set(f.fuse());
        }

        let output = ready!(this.f.poll(cx));
        let item = output??;
        *this.offset += item.len() as u64;
        Poll::Ready(Ok(item))
    }
}
