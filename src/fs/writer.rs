use crate::buf::Buf;
use crate::runtime::Runtime;
use std::cmp;
use std::fs::File;
use std::future::Future;
use std::io;
use std::io::IoSlice;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

#[pin_project::pin_project]
pub(crate) struct Writer<R>
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

impl<R> Writer<R>
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

    pub(crate) fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll(cx, |buf| {
            let n = bufs
                .iter()
                .map(|b| {
                    let n = cmp::min(buf.capacity() - buf.len(), b.len());
                    buf.extend_from_slice(&b[..n]);
                    n
                })
                .sum();
            (n > 0).then_some(n)
        })
    }

    pub(crate) fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(cx, |buf| buf.is_empty().then_some(()))
    }

    fn poll<F, T>(self: Pin<&mut Self>, cx: &mut Context<'_>, mut f: F) -> Poll<io::Result<T>>
    where
        F: FnMut(&mut Buf) -> Option<T>,
    {
        let mut this = self.project();
        loop {
            match this.state.as_mut().project() {
                StateProj::Idle(buf) => {
                    let mut buf = buf
                        .take()
                        .unwrap_or_else(|| Buf::with_capacity(*this.capacity));
                    let output = f(&mut buf);
                    if buf.is_empty() {
                        this.state.set(State::Idle(Some(buf)));
                    } else {
                        let file = this.file.clone();
                        let offset = *this.offset;
                        let f = this.runtime.spawn_blocking(move || {
                            let (head, tail) = buf.as_slices();
                            match crate::unstable::write_vectored_at(
                                &file,
                                &[IoSlice::new(head), IoSlice::new(tail)],
                                offset,
                            ) {
                                Ok(n) => {
                                    buf.consume(n);
                                    (buf, n, None)
                                }
                                Err(e) => (buf, 0, Some(e)),
                            }
                        });
                        this.state.set(State::Busy(f));
                    }
                    if let Some(output) = output {
                        break Poll::Ready(Ok(output));
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
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::Runtime;
    use std::future;
    use std::io::{self, BufReader, IoSlice, Read};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    async fn write_all<R>(mut writer: Pin<&mut super::Writer<R>>, mut buf: &[u8]) -> io::Result<()>
    where
        R: Runtime,
    {
        while !buf.is_empty() {
            let n = future::poll_fn(|cx| {
                writer
                    .as_mut()
                    .poll_write_vectored(cx, &[IoSlice::new(buf)])
            })
            .await?;

            buf = &buf[n..];
        }
        Ok(())
    }

    async fn flush<R>(mut writer: Pin<&mut super::Writer<R>>) -> io::Result<()>
    where
        R: Runtime,
    {
        future::poll_fn(|cx| writer.as_mut().poll_flush(cx)).await
    }

    #[tokio::test]
    async fn test() {
        let file = Arc::new(tempfile::tempfile().unwrap());

        let mut reader = &*file;
        let mut writer = pin::pin!(super::Writer::new(
            tokio::runtime::Handle::current(),
            67,
            file.clone()
        ));

        let mut buf = Vec::new();

        write_all(writer.as_mut(), b"hello").await.unwrap();
        flush(writer.as_mut()).await.unwrap();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");

        write_all(writer.as_mut(), b" world").await.unwrap();
        flush(writer.as_mut()).await.unwrap();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello world");
    }

    #[tokio::test]
    async fn test_random() {
        let file = Arc::new(tempfile::tempfile().unwrap());
        let data = crate::tests::random();

        let read = tokio::task::spawn_blocking({
            let file = file.clone();
            let data = data.clone();
            move || {
                let mut reader = BufReader::new(&*file);
                let mut buf = [0; 1];
                for b in &*data {
                    loop {
                        let n = reader.read(&mut buf).unwrap();
                        if n == 1 {
                            assert_eq!(buf[0], *b);
                            break;
                        }
                    }
                }
            }
        });

        let write = tokio::spawn({
            let file = file.clone();
            let data = data.clone();
            async move {
                let mut writer = pin::pin!(super::Writer::new(
                    tokio::runtime::Handle::current(),
                    67,
                    file.clone()
                ));
                for chunk in data.chunks(257) {
                    write_all(writer.as_mut(), chunk).await.unwrap();
                }
                flush(writer.as_mut()).await.unwrap();
            }
        });

        futures::future::try_join(read, write).await.unwrap();
    }
}
