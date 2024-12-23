use crate::buf::Buf;
use crate::runtime::Runtime;
use std::fs::File;
use std::future::Future;
use std::io::IoSliceMut;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::{cmp, io};

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
    Idle(Option<Buf>),
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
            state: State::Idle(None),
        }
    }

    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }

    pub(crate) fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll(cx, |buf| {
            let len = buf.len();
            for b in bufs {
                let (data, _) = buf.as_slices();
                let n = cmp::min(b.len(), data.len());
                b[..n].copy_from_slice(&data[..n]);
                b.advance(n);
                buf.consume(n);

                let (data, _) = buf.as_slices();
                let n = cmp::min(b.len(), data.len());
                b[..n].copy_from_slice(&data[..n]);
                b.advance(n);
                buf.consume(n);
            }
            len - buf.len()
        })
        .map_ok(Option::unwrap_or_default)
    }

    fn poll<F, T>(self: Pin<&mut Self>, cx: &mut Context<'_>, f: F) -> Poll<io::Result<Option<T>>>
    where
        F: FnOnce(&mut Buf) -> T,
    {
        let mut this = self.project();
        loop {
            match this.state.as_mut().project() {
                StateProj::Idle(buf) => {
                    let mut buf = buf
                        .take()
                        .unwrap_or_else(|| Buf::with_capacity(*this.capacity));
                    if buf.is_empty() {
                        let file = this.file.clone();
                        let offset = *this.offset;
                        let f = this.runtime.spawn_blocking(move || unsafe {
                            let (head, tail) = buf.spare_capacity_mut();
                            match crate::unstable::read_vectored_at(
                                &file,
                                &mut [
                                    IoSliceMut::new(crate::unstable::slice_assume_init_mut(head)),
                                    IoSliceMut::new(crate::unstable::slice_assume_init_mut(tail)),
                                ],
                                offset,
                            ) {
                                Ok(n) => {
                                    buf.set_init(n);
                                    (buf, n, None)
                                }
                                Err(e) => (buf, 0, Some(e)),
                            }
                        });
                        this.state.set(State::Busy(f));
                    } else {
                        let output = f(&mut buf);
                        this.state.set(State::Idle(Some(buf)));
                        break Poll::Ready(Ok(Some(output)));
                    }
                }
                StateProj::Busy(f) => {
                    let (buf, n, e) = match ready!(f.poll(cx)) {
                        Ok((buf, n, e)) => (Some(buf), n, e),
                        Err(e) => (None, 0, Some(e)),
                    };
                    *this.offset += n as u64;
                    this.state.set(State::Idle(buf));
                    if let Some(e) = e {
                        break Poll::Ready(Err(e));
                    } else if n == 0 {
                        break Poll::Ready(Ok(None));
                    }
                }
            }
        }
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
                State::Busy(_) => State::Idle(None),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::Runtime;
    use std::future;
    use std::io::{self, BufWriter, IoSliceMut, Write};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    async fn read<R>(mut reader: Pin<&mut super::Reader<R>>, buf: &mut [u8]) -> io::Result<usize>
    where
        R: Runtime,
    {
        future::poll_fn(|cx| {
            reader
                .as_mut()
                .poll_read_vectored(cx, &mut [IoSliceMut::new(buf)])
        })
        .await
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
