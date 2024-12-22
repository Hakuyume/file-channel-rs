use crate::runtime::SpawnBlocking;
use slab::Slab;
use std::io::{self, IoSlice, IoSliceMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll, Waker};

#[pin_project::pin_project(PinnedDrop)]
pub struct Reader<R>
where
    R: SpawnBlocking,
{
    #[pin]
    reader: crate::fs::Reader<R>,
    inner: Arc<Mutex<Inner>>,
    key: usize,
}

impl<R> Reader<R>
where
    R: SpawnBlocking,
{
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        loop {
            let n = ready!(this.reader.as_mut().poll_read_vectored(cx, bufs))?;
            if n > 0 {
                break Poll::Ready(Ok(n));
            } else {
                let mut inner = this.inner.lock().unwrap();
                if this.reader.offset() >= inner.len {
                    match inner.state {
                        State::Open => {
                            inner.wakers[*this.key] = Some(cx.waker().clone());
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
        let mut inner = self.inner.lock().unwrap();
        inner.wakers.remove(self.key);
    }
}

#[pin_project::pin_project(PinnedDrop, project = WriterProj)]
pub struct Writer<R>
where
    R: SpawnBlocking,
{
    #[pin]
    writer: crate::fs::Writer<R>,
    inner: Arc<Mutex<Inner>>,
}

impl<R> Writer<R>
where
    R: SpawnBlocking,
{
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        let n = ready!(this.writer.as_mut().poll_write_vectored(cx, bufs))?;
        this.sync(None);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        ready!(this.writer.as_mut().poll_flush(cx))?;
        this.sync(None);
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        ready!(this.writer.as_mut().poll_flush(cx))?;
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
        let mut inner = self.inner.lock().unwrap();
        let mut is_modified = false;
        if inner.len < self.writer.offset() {
            inner.len = self.writer.offset();
            is_modified |= true;
        }
        if let (State::Open, Some(state)) = (inner.state, state) {
            inner.state = state;
            is_modified |= true;
        }
        if is_modified {
            for (_, waker) in &mut inner.wakers {
                if let Some(waker) = waker.take() {
                    waker.wake();
                }
            }
        }
    }
}

struct Inner {
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

#[cfg(feature = "futures-io")]
const _: () = {
    impl<R> futures::io::AsyncRead for Reader<R>
    where
        R: SpawnBlocking,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.poll_read_vectored(cx, &mut [IoSliceMut::new(buf)])
        }
        fn poll_read_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
        ) -> Poll<io::Result<usize>> {
            self.poll_read_vectored(cx, bufs)
        }
    }

    impl<R> futures::io::AsyncWrite for Writer<R>
    where
        R: SpawnBlocking,
    {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.poll_write_vectored(cx, &[IoSlice::new(buf)])
        }
        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.poll_flush(cx)
        }
        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.poll_close(cx)
        }
        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.poll_write_vectored(cx, bufs)
        }
    }
};
