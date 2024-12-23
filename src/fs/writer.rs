use crate::buf::Buf;
use crate::runtime::Runtime;
use bytes::Buf as _;
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

    pub(crate) fn poll<F, T>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut f: F,
    ) -> Poll<io::Result<T>>
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
                    if buf.has_remaining() {
                        let file = this.file.clone();
                        let offset = *this.offset;
                        let f = this.runtime.spawn_blocking(move || {
                            let [chunk0, chunk1] = buf.chunks();
                            match crate::unstable::write_vectored_at(
                                &file,
                                &[IoSlice::new(chunk0), IoSlice::new(chunk1)],
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
                        this.state.set(State::Idle(Some(buf)));
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
    use bytes::{Buf, BufMut};
    use std::cmp;
    use std::future;
    use std::io::{self, BufReader, Read};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    async fn write_all<R>(mut writer: Pin<&mut super::Writer<R>>, mut buf: &[u8]) -> io::Result<()>
    where
        R: Runtime,
    {
        future::poll_fn(|cx| {
            writer.as_mut().poll(cx, |b| {
                let n = cmp::min(b.remaining_mut(), buf.len());
                b.put(&buf[..n]);
                buf = &buf[n..];
                (!b.has_remaining()).then_some(())
            })
        })
        .await
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
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");

        write_all(writer.as_mut(), b" world").await.unwrap();
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
            }
        });

        futures::future::try_join(read, write).await.unwrap();
    }
}
