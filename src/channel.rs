use crate::runtime::SpawnBlocking;
use futures::FutureExt;
use slab::Slab;
use std::fs::File;
use std::future::Future;
use std::io::{self, IoSlice, IoSliceMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{ready, Context, Poll, Waker};

pub fn tempfile<R>(mut runtime: R) -> impl Future<Output = io::Result<(Writer<R>, Reader<R>)>>
where
    R: Clone + SpawnBlocking,
{
    runtime
        .spawn_blocking(tempfile::tempfile)
        .map(move |output| Ok(file(runtime, crate::fs::DEFAULT_BUF_SIZE, output??)))
}

fn file<R>(runtime: R, capacity: usize, file: File) -> (Writer<R>, Reader<R>)
where
    R: Clone + SpawnBlocking,
{
    let file = Arc::new(file);

    let mut shared = Shared {
        len: 0,
        state: State::Open,
        wakers: Slab::new(),
    };
    let key = shared.wakers.insert(None);
    let shared = Arc::new(Mutex::new(shared));

    (
        Writer {
            inner: crate::fs::Writer::new(runtime.clone(), capacity, file.clone()),
            shared: shared.clone(),
        },
        Reader {
            inner: crate::fs::Reader::new(runtime, capacity, file),
            shared,
            key,
        },
    )
}

#[pin_project::pin_project(PinnedDrop)]
pub struct Reader<R>
where
    R: SpawnBlocking,
{
    #[pin]
    inner: crate::fs::Reader<R>,
    shared: Arc<Mutex<Shared>>,
    key: usize,
}

impl<R> Reader<R>
where
    R: SpawnBlocking,
{
    pub(crate) fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        loop {
            let n = ready!(this.inner.as_mut().poll_read_vectored(cx, bufs))?;
            if n > 0 {
                break Poll::Ready(Ok(n));
            } else {
                let mut shared = lock(this.shared);
                if this.inner.offset() >= shared.len {
                    match shared.state {
                        State::Open => {
                            shared.wakers[*this.key] = Some(cx.waker().clone());
                            break Poll::Pending;
                        }
                        State::Close => break Poll::Ready(Ok(0)),
                        State::Abort => break Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
                    }
                }
            }
        }
    }
}

#[pin_project::pinned_drop]
impl<R> PinnedDrop for Reader<R>
where
    R: SpawnBlocking,
{
    fn drop(self: Pin<&mut Self>) {
        lock(&self.shared).wakers.remove(self.key);
    }
}

impl<R> Clone for Reader<R>
where
    R: Clone + SpawnBlocking,
{
    fn clone(&self) -> Self {
        let key = lock(&self.shared).wakers.insert(None);
        Self {
            inner: self.inner.clone(),
            shared: self.shared.clone(),
            key,
        }
    }
}

#[pin_project::pin_project(PinnedDrop, project = WriterProj)]
pub struct Writer<R>
where
    R: SpawnBlocking,
{
    #[pin]
    inner: crate::fs::Writer<R>,
    shared: Arc<Mutex<Shared>>,
}

impl<R> Writer<R>
where
    R: SpawnBlocking,
{
    pub(crate) fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        let n = ready!(this.inner.as_mut().poll_write_vectored(cx, bufs))?;
        this.sync(None);
        Poll::Ready(Ok(n))
    }

    pub(crate) fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        ready!(this.inner.as_mut().poll_flush(cx))?;
        this.sync(None);
        Poll::Ready(Ok(()))
    }

    pub(crate) fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        ready!(this.inner.as_mut().poll_flush(cx))?;
        this.sync(Some(State::Close));
        Poll::Ready(Ok(()))
    }
}

#[pin_project::pinned_drop]
impl<R> PinnedDrop for Writer<R>
where
    R: SpawnBlocking,
{
    fn drop(self: Pin<&mut Self>) {
        self.project().sync(Some(State::Abort))
    }
}

impl<R> WriterProj<'_, R>
where
    R: SpawnBlocking,
{
    fn sync(&mut self, state: Option<State>) {
        let mut shared = lock(self.shared);
        let mut is_modified = false;
        if shared.len < self.inner.offset() {
            shared.len = self.inner.offset();
            is_modified |= true;
        }
        if let (State::Open, Some(state)) = (shared.state, state) {
            shared.state = state;
            is_modified |= true;
        }
        if is_modified {
            for (_, waker) in &mut shared.wakers {
                if let Some(waker) = waker.take() {
                    waker.wake();
                }
            }
        }
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap()
}

struct Shared {
    len: u64,
    state: State,
    wakers: Slab<Option<Waker>>,
}

#[derive(Clone, Copy)]
enum State {
    Open,
    Close,
    Abort,
}
