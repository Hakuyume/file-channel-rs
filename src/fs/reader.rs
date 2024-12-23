use crate::buf::Buf;
use crate::runtime::Runtime;
use std::fs::File;
use std::future::Future;
use std::io;
use std::io::IoSliceMut;
use std::mem;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

#[pin_project::pin_project]
pub(crate) struct Reader<R>
where
    R: Runtime,
{
    runtime: R,
    capacity: usize,
    file: Arc<File>,
    offset: u64,
    #[pin]
    state: State<R::Future<Output>>,
}

#[pin_project::pin_project(project = StateProj)]
enum State<F> {
    Idle(Buf),
    Busy(#[pin] F),
}

type Output = (Buf, usize, Option<io::Error>);

impl<R> Reader<R>
where
    R: Runtime,
{
    pub(crate) fn new(runtime: R, capacity: usize, file: Arc<File>) -> Self {
        Self {
            runtime,
            capacity,
            file,
            offset: 0,
            state: State::Idle(Buf::default()),
        }
    }

    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }

    pub(crate) fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        loop {
            match this.state.as_mut().project() {
                StateProj::Idle(buf) => {
                    if buf.is_empty() {
                        let file = this.file.clone();
                        let offset = *this.offset;
                        let mut buf = if buf.capacity() > 0 {
                            mem::take(buf)
                        } else {
                            Buf::with_capacity(*this.capacity)
                        };
                        let f = this.runtime.spawn_blocking(move || unsafe {
                            let (head, tail) = buf.unfilled();
                            match crate::unstable::read_vectored_at(
                                &file,
                                &mut [
                                    IoSliceMut::new(crate::unstable::slice_assume_init_mut(head)),
                                    IoSliceMut::new(crate::unstable::slice_assume_init_mut(tail)),
                                ],
                                offset,
                            ) {
                                Ok(n) => {
                                    buf.advance(n);
                                    (buf, n, None)
                                }
                                Err(e) => (buf, 0, Some(e)),
                            }
                        });
                        this.state.set(State::Busy(f));
                    } else {
                        break Poll::Ready(Ok(buf.len()));
                    }
                }
                StateProj::Busy(f) => {
                    let (buf, n, e) = match ready!(f.poll(cx)) {
                        Ok((buf, n, e)) => (buf, n, e),
                        Err(e) => (Buf::default(), 0, Some(e)),
                    };
                    *this.offset += n as u64;
                    this.state.set(State::Idle(buf));
                    if let Some(e) = e {
                        break Poll::Ready(Err(e));
                    } else if n == 0 {
                        break Poll::Ready(Ok(0));
                    }
                }
            }
        }
    }

    pub(crate) fn buf(self: Pin<&mut Self>) -> &mut Buf {
        let this = self.project();
        let StateProj::Idle(buf) = this.state.project() else {
            panic!()
        };
        buf
    }
}

impl<R> Clone for Reader<R>
where
    R: Clone + Runtime,
{
    fn clone(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
            capacity: self.capacity,
            file: self.file.clone(),
            offset: self.offset,
            state: match &self.state {
                State::Idle(buf) => State::Idle(buf.clone()),
                State::Busy(_) => State::Idle(Buf::default()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::Runtime;
    use std::cmp;
    use std::future;
    use std::io::{self, BufWriter, Write};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    async fn read<R>(mut reader: Pin<&mut super::Reader<R>>, buf: &mut [u8]) -> io::Result<usize>
    where
        R: Runtime,
    {
        future::poll_fn(|cx| reader.as_mut().poll(cx)).await?;
        let b = reader.buf();
        let (data, _) = b.filled();
        let n = cmp::min(buf.len(), data.len());
        buf[..n].copy_from_slice(&data[..n]);
        b.consume(n);
        Ok(n)
    }

    #[tokio::test]
    async fn test() {
        async fn assert<R>(mut reader: Pin<&mut super::Reader<R>>, mut expected: &[u8])
        where
            R: Runtime,
        {
            let mut buf = [0; 3];
            loop {
                let n = read(reader.as_mut(), &mut buf).await.unwrap();
                if n == 0 {
                    break;
                } else {
                    assert_eq!(buf[..n], expected[..n]);
                    expected = &expected[n..];
                }
            }
            assert!(expected.is_empty());
        }

        let file = Arc::new(tempfile::tempfile().unwrap());
        let mut writer = &*file;

        let mut reader_0 = pin::pin!(super::Reader::new(
            tokio::runtime::Handle::current(),
            67,
            file.clone()
        ));
        assert(reader_0.as_mut(), b"").await;

        writer.write_all(b"hello").unwrap();
        writer.flush().unwrap();
        let mut reader_1 = pin::pin!(reader_0.clone());
        assert(reader_1.as_mut(), b"hello").await;

        let mut reader_2 = pin::pin!(reader_1.clone());
        assert(reader_1.as_mut(), b"").await;
        assert(reader_2.as_mut(), b"").await;

        writer.write_all(b" world").unwrap();
        writer.flush().unwrap();
        let mut reader_3 = pin::pin!(reader_0.clone());
        assert(reader_0.as_mut(), b"hello world").await;
        assert(reader_1.as_mut(), b" world").await;
        assert(reader_2.as_mut(), b" world").await;
        assert(reader_3.as_mut(), b"hello world").await;
    }

    #[tokio::test]
    async fn test_random() {
        let file = Arc::new(tempfile::tempfile().unwrap());
        let data = crate::tests::random();

        let read = tokio::spawn({
            let file = file.clone();
            let data = data.clone();
            async move {
                let mut reader = pin::pin!(super::Reader::new(
                    tokio::runtime::Handle::current(),
                    67,
                    file.clone()
                ));
                let mut buf = [0; 1];
                for b in &*data {
                    loop {
                        let n = read(reader.as_mut(), &mut buf).await.unwrap();
                        if n == 1 {
                            assert_eq!(buf[0], *b);
                            break;
                        }
                    }
                }
            }
        });

        let write = tokio::task::spawn_blocking({
            let file = file.clone();
            let data = data.clone();
            move || {
                let mut writer = BufWriter::new(&*file);
                for chunk in data.chunks(257) {
                    writer.write_all(chunk).unwrap();
                }
                writer.flush().unwrap();
            }
        });

        futures::future::try_join(read, write).await.unwrap();
    }
}
