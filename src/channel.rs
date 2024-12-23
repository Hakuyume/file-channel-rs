use crate::runtime::SpawnBlocking;
use futures::FutureExt;
use slab::Slab;
use std::fs::File;
use std::future::Future;
use std::io::{self, IoSlice, IoSliceMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
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

#[pin_project::pin_project]
pub struct Reader<R>
where
    R: SpawnBlocking,
{
    #[pin]
    inner: crate::fs::Reader<R>,
    offset: Arc<AtomicU64>,
    guard: ReaderGuard,
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
                        break Poll::Ready(Ok(0));
                    }
                }
            }
        }
    }
}

impl<R> Clone for Reader<R>
where
    R: Clone + SpawnBlocking,
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

#[pin_project::pin_project]
pub struct Writer<R>
where
    R: SpawnBlocking,
{
    #[pin]
    inner: crate::fs::Writer<R>,
    offset: Arc<AtomicU64>,
    wakers: Option<Arc<WakerSet>>,
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
        self.poll(cx, |cx, inner| inner.poll_write_vectored(cx, bufs))
    }

    pub(crate) fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(cx, |cx, inner| inner.poll_flush(cx))
    }

    pub(crate) fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll(cx, |cx, inner| inner.poll_flush(cx)))?;
        *self.project().wakers = None;
        Poll::Ready(Ok(()))
    }

    fn poll<F, T>(self: Pin<&mut Self>, cx: &mut Context<'_>, f: F) -> Poll<io::Result<T>>
    where
        F: FnOnce(&mut Context<'_>, Pin<&mut crate::fs::Writer<R>>) -> Poll<io::Result<T>>,
    {
        let mut this = self.project();
        let WakerSet(wakers) = this.wakers.as_deref().ok_or(io::ErrorKind::NotConnected)?;
        let output = ready!(f(cx, this.inner.as_mut()))?;
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
        Poll::Ready(Ok(output))
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
