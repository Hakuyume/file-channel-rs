use futures::Sink;
use std::fs::File;
use std::future::Future;
use std::io;
use std::mem;
use std::os::unix::fs::FileExt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use crate::runtime;

#[pin_project::pin_project(!Unpin)]
pub struct Sender(#[pin] Inner, Guard);

impl Sender {
    pub(crate) fn new(
        capacity: usize,
        file: Arc<File>,
        state: Arc<Mutex<crate::state::State>>,
    ) -> Self {
        Self(Inner::new(capacity, file), Guard { state })
    }

    fn poll<F>(self: Pin<&mut Self>, f: F) -> Poll<io::Result<()>>
    where
        F: FnOnce(Pin<&mut Inner>) -> Poll<io::Result<()>>,
    {
        let mut this = self.project();
        ready!(f(this.0.as_mut()))?;
        {
            let mut state = this.1.state.lock().unwrap();
            if mem::replace(&mut state.offset, this.0.offset()) != this.0.offset() {
                for (_, waker) in &mut state.wakers {
                    if let Some(waker) = waker.take() {
                        waker.wake();
                    }
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<Item> Sink<Item> for Sender
where
    Item: AsRef<[u8]>,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll(|inner| inner.poll_ready(cx))
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error> {
        let this = self.project();
        this.0.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll(|inner| inner.poll_flush(cx))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll(|inner| inner.poll_flush(cx)))?;
        {
            let mut state = self.1.state.lock().unwrap();
            if !mem::replace(&mut state.is_closed, true) {
                for (_, waker) in &mut state.wakers {
                    if let Some(waker) = waker.take() {
                        waker.wake();
                    }
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

struct Guard {
    state: Arc<Mutex<crate::state::State>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if !mem::replace(&mut state.is_closed, true) {
                for (_, waker) in &mut state.wakers {
                    if let Some(waker) = waker.take() {
                        waker.wake();
                    }
                }
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
    state: State<runtime::SpawnBlocking<Output>>,
}

#[pin_project::pin_project(project = StateProj)]
enum State<F> {
    Idle(Option<Vec<u8>>),
    Busy(#[pin] F),
}

type Output = (Vec<u8>, Option<io::Error>);

impl Inner {
    fn new(capacity: usize, file: Arc<File>) -> Self {
        Self {
            capacity,
            file,
            offset: 0,
            state: State::Idle(None),
        }
    }

    fn offset(&self) -> u64 {
        self.offset
    }

    fn spawn<F>(self: Pin<&mut Self>, f: F)
    where
        F: FnOnce(usize) -> bool,
    {
        let mut this = self.project();
        match this.state.as_mut().project() {
            StateProj::Idle(buf) => {
                if let Some(buf) = buf.take() {
                    if f(buf.len()) {
                        let file = this.file.clone();
                        let offset = *this.offset;
                        let f = runtime::spawn_blocking(move || {
                            let e = file.write_all_at(&buf, offset).err();
                            (buf, e)
                        });
                        this.state.set(State::Busy(f));
                    } else {
                        this.state.set(State::Idle(Some(buf)));
                    }
                }
            }
            StateProj::Busy(_) => (),
        }
    }

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        match this.state.as_mut().project() {
            StateProj::Idle(_) => Poll::Ready(Ok(())),
            StateProj::Busy(f) => {
                let (buf, e) = match ready!(f.poll(cx)) {
                    Ok((mut buf, e)) => {
                        if e.is_none() {
                            *this.offset += buf.len() as u64;
                            buf.clear();
                        }
                        (Some(buf), e)
                    }
                    Err(e) => (None, Some(e)),
                };
                this.state.set(State::Idle(buf));
                if let Some(e) = e {
                    Poll::Ready(Err(e))
                } else {
                    Poll::Ready(Ok(()))
                }
            }
        }
    }

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(cx)
    }

    fn start_send<Item>(mut self: Pin<&mut Self>, item: Item) -> io::Result<()>
    where
        Item: AsRef<[u8]>,
    {
        let capacity = self.capacity;

        let this = self.as_mut().project();
        let StateProj::Idle(buf) = this.state.project() else {
            panic!("not ready")
        };
        let buf = buf.get_or_insert_with(|| Vec::with_capacity(capacity * 2));
        buf.extend_from_slice(item.as_ref());

        self.spawn(|len| len >= capacity);

        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.as_mut().spawn(|len| len > 0);
        self.poll(cx)
    }
}
