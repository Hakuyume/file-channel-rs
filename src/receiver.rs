use crate::runtime;
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
pub struct Receiver(#[pin] Inner, Guard);

impl Receiver {
    pub(crate) fn new(
        capacity: usize,
        file: Arc<File>,
        state: Arc<Mutex<crate::state::State>>,
    ) -> Self {
        Self(Inner::new(capacity, file), Guard { state, key: None })
    }
}

impl Stream for Receiver {
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
struct Inner {
    capacity: usize,
    file: Arc<File>,
    offset: u64,
    #[pin]
    f: Fuse<runtime::SpawnBlocking<io::Result<Vec<u8>>>>,
}

impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            capacity: self.capacity,
            file: self.file.clone(),
            offset: self.offset,
            f: Fuse::terminated(),
        }
    }
}

impl Inner {
    fn new(capacity: usize, file: Arc<File>) -> Self {
        Self {
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
            let f = runtime::spawn_blocking(move || {
                let mut buf = Vec::with_capacity(capacity);
                unsafe {
                    let n = file.read_at(
                        // https://github.com/rust-lang/rust/issues/63569
                        &mut *(buf.spare_capacity_mut() as *mut [_] as *mut [_]),
                        offset,
                    )?;
                    buf.set_len(n);
                }
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
