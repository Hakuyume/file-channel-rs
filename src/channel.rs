use crate::buf::Buf;
use crate::runtime::Runtime;
use bytes::{Buf as _, BufMut};
use futures::FutureExt;
use slab::Slab;
use std::fs::File;
use std::future::Future;
use std::io::{self, IoSlice, IoSliceMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{ready, Context, Poll, Waker};

// https://github.com/rust-lang/rust/blob/426d1734238e3c5f52e935ba4f617f3a9f43b59d/library/std/src/sys_common/io.rs#L3
const DEFAULT_BUF_SIZE: usize = 8 * 1024;

pub fn tempfile<R>(runtime: R) -> impl Future<Output = io::Result<(Writer<R>, Reader<R>)>>
where
    R: Clone + Runtime,
{
    runtime
        .spawn_blocking(tempfile::tempfile)
        .map(move |output| Ok(file(runtime, DEFAULT_BUF_SIZE, output??)))
}

fn file<R>(runtime: R, capacity: usize, file: File) -> (Writer<R>, Reader<R>)
where
    R: Clone + Runtime,
{
    let file = Arc::new(file);

    let offset = Arc::new(AtomicU64::new(0));
    let wakers = Arc::default();

    let reader = Reader {
        inner: crate::fs::Reader::new(runtime.clone(), capacity, file.clone()),
        offset: offset.clone(),
        guard: ReaderGuard(Arc::downgrade(&wakers), None),
    };
    let writer = Writer {
        inner: crate::fs::Writer::new(runtime.clone(), capacity, file.clone()),
        offset,
        wakers: Some(wakers),
    };

    (writer, reader)
}

#[pin_project::pin_project(!Unpin)]
pub struct Reader<R>
where
    R: Runtime,
{
    #[pin]
    inner: crate::fs::Reader<R>,
    offset: Arc<AtomicU64>,
    guard: ReaderGuard,
}

impl<R> Reader<R>
where
    R: Runtime,
{
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        loop {
            ready!(this.inner.as_mut().poll(cx))?;
            if this.inner.as_mut().buf().has_remaining() {
                break Poll::Ready(Ok(()));
            } else if this.inner.offset() >= this.offset.load(Ordering::SeqCst) {
                let is_registered =
                    if let Some(WakerSet(wakers)) = this.guard.0.upgrade().as_deref() {
                        if let Ok(mut wakers) = wakers.lock() {
                            let key = this.guard.1.get_or_insert_with(|| wakers.insert(None));
                            wakers[*key] = Some(cx.waker().clone());
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                if this.inner.offset() >= this.offset.load(Ordering::SeqCst) {
                    if is_registered {
                        break Poll::Pending;
                    } else {
                        break Poll::Ready(Ok(()));
                    }
                }
            }
        }
    }

    fn buf(self: Pin<&mut Self>) -> &mut Buf {
        let this = self.project();
        this.inner.buf()
    }
}

impl<R> Clone for Reader<R>
where
    R: Clone + Runtime,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            offset: self.offset.clone(),
            guard: ReaderGuard(self.guard.0.clone(), None),
        }
    }
}

struct ReaderGuard(Weak<WakerSet>, Option<usize>);
impl Drop for ReaderGuard {
    fn drop(&mut self) {
        if let (Some(WakerSet(wakers)), Some(key)) = (self.0.upgrade().as_deref(), self.1) {
            if let Ok(mut wakers) = wakers.lock() {
                wakers.remove(key);
            }
        }
    }
}

#[pin_project::pin_project(!Unpin)]
pub struct Writer<R>
where
    R: Runtime,
{
    #[pin]
    inner: crate::fs::Writer<R>,
    offset: Arc<AtomicU64>,
    wakers: Option<Arc<WakerSet>>,
}

impl<R> Writer<R>
where
    R: Runtime,
{
    fn poll<F, T>(self: Pin<&mut Self>, cx: &mut Context<'_>, f: F) -> Poll<io::Result<T>>
    where
        F: FnMut(&mut Buf) -> Option<T>,
    {
        let mut this = self.project();
        let WakerSet(wakers) = this.wakers.as_deref().ok_or(io::ErrorKind::NotConnected)?;
        let output = this.inner.as_mut().poll(cx, f);
        let offset = this.offset.swap(this.inner.offset(), Ordering::SeqCst);
        if offset < this.inner.offset() {
            if let Ok(mut wakers) = wakers.lock() {
                for (_, waker) in &mut *wakers {
                    if let Some(waker) = waker.take() {
                        waker.wake();
                    }
                }
            }
        }
        output
    }
}

#[derive(Default)]
struct WakerSet(Mutex<Slab<Option<Waker>>>);
impl Drop for WakerSet {
    fn drop(&mut self) {
        if let Ok(wakers) = self.0.get_mut() {
            for (_, waker) in wakers {
                if let Some(waker) = waker.take() {
                    waker.wake();
                }
            }
        }
    }
}

impl<R> futures::io::AsyncRead for Reader<R>
where
    R: Runtime,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_vectored(cx, &mut [IoSliceMut::new(buf)])
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        ready!(self.as_mut().poll(cx))?;
        let buf = self.buf();
        let remaining = buf.remaining();
        for b in bufs {
            let mut b = &mut b[..];
            b.put(buf.take(b.remaining_mut()));
        }
        let n = remaining - buf.remaining();
        Poll::Ready(Ok(n))
    }
}

impl<R> futures::io::AsyncWrite for Writer<R>
where
    R: Runtime,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored(cx, &[IoSlice::new(buf)])
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(cx, |buf| (!buf.has_remaining()).then_some(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        *self.project().wakers = None;
        Poll::Ready(Ok(()))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll(cx, |buf| {
            let remaining_mut = buf.remaining_mut();
            for b in bufs {
                buf.put(b.take(buf.remaining_mut()));
            }
            let n = remaining_mut - buf.remaining_mut();
            (n > 0).then_some(n)
        })
    }
}

#[cfg(feature = "tokio")]
const _: () = {
    impl<R> tokio::io::AsyncRead for Reader<R>
    where
        R: Runtime,
    {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            ready!(self.as_mut().poll(cx))?;
            let b = self.buf();
            buf.put(b.take(buf.remaining_mut()));
            Poll::Ready(Ok(()))
        }
    }

    impl<R> tokio::io::AsyncWrite for Writer<R>
    where
        R: Runtime,
    {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            futures::io::AsyncWrite::poll_write(self, cx, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            futures::io::AsyncWrite::poll_flush(self, cx)
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            futures::io::AsyncWrite::poll_close(self, cx)
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<Result<usize, io::Error>> {
            futures::io::AsyncWrite::poll_write_vectored(self, cx, bufs)
        }

        fn is_write_vectored(&self) -> bool {
            true
        }
    }
};
