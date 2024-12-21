use super::buf::Buf;
use super::runtime::{spawn_blocking, SpawnBlocking};
use std::fs::File;
use std::future::Future;
use std::io;
use std::io::IoSlice;
use std::os::unix::fs::FileExt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

#[pin_project::pin_project]
pub(crate) struct Writer {
    file: Arc<File>,
    offset: u64,
    #[pin]
    state: State,
}

#[pin_project::pin_project(project = StateProj)]
enum State {
    Idle(Option<Buf>),
    Busy(#[pin] SpawnBlocking<(Buf, usize, Option<io::Error>)>),
}

impl Writer {
    pub(crate) fn from_file(file: Arc<File>) -> Self {
        Self {
            file,
            offset: 0,
            state: State::Idle(None),
        }
    }

    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }

    pub(crate) fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored(cx, &[IoSlice::new(buf)])
    }

    pub(crate) fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll(cx, |buf| {
            let n = bufs.iter().map(|buf| buf.len()).sum();
            buf.reserve(n);
            for b in bufs {
                buf.extend_from_slice(b);
            }
            n
        })
    }

    pub(crate) fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll(cx, |_| ())
    }

    fn poll<F, T>(self: Pin<&mut Self>, cx: &mut Context<'_>, f: F) -> Poll<io::Result<T>>
    where
        F: FnOnce(&mut Buf) -> T,
    {
        let mut this = self.project();
        let mut f = Some(f);
        loop {
            match this.state.as_mut().project() {
                StateProj::Idle(buf) => {
                    let mut buf = buf.take().unwrap_or_default();

                    let output = if buf.is_empty() {
                        Some(f.take().unwrap()(&mut buf))
                    } else {
                        None
                    };

                    let file = this.file.clone();
                    let offset = *this.offset;
                    let f = spawn_blocking(move || match file.write_at(&buf, offset) {
                        Ok(n) => {
                            buf.advance(n);
                            (buf, n, None)
                        }
                        Err(e) => (buf, 0, Some(e)),
                    });
                    this.state.set(State::Busy(f));

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
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};
    use std::future;
    use std::io::{self, Read};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    #[tokio::test]
    async fn test() {
        async fn write(writer: &mut Pin<&mut super::Writer>, mut buf: &[u8]) -> io::Result<()> {
            while !buf.is_empty() {
                let n = future::poll_fn(|cx| writer.as_mut().poll_write(cx, &mut buf)).await?;
                buf = &buf[n..];
            }
            future::poll_fn(|cx| writer.as_mut().poll_flush(cx)).await
        }

        let file = Arc::new(tempfile::tempfile().unwrap());

        let mut writer = pin::pin!(super::Writer::from_file(file.clone()));

        let mut buf = Vec::new();
        write(&mut writer, b"hello").await.unwrap();
        (&mut &*file).read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        write(&mut writer, b" world").await.unwrap();
        (&mut &*file).read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello world");
    }

    #[tokio::test]
    async fn test_random() {
        let file = Arc::new(tempfile::tempfile().unwrap());
        let rng = StdRng::seed_from_u64(42);

        let read = tokio::task::spawn_blocking({
            let file = file.clone();
            let mut rng = rng.clone();
            move || {
                let mut buf = vec![0; 4096];
                let mut buf_expected = vec![0; 4096];
                for _ in 0..4096 {
                    {
                        let mut buf = &mut buf[..];
                        while !buf.is_empty() {
                            let n = (&mut &*file).read(buf).unwrap();
                            buf = &mut buf[n..];
                        }
                    }
                    rng.fill_bytes(&mut buf_expected);
                    assert_eq!(buf, buf_expected);
                }
            }
        });

        let write = tokio::spawn({
            let file = file.clone();
            let mut rng = rng.clone();
            async move {
                let mut writer = pin::pin!(super::Writer::from_file(file.clone()));
                let mut buf = vec![0; 4096];
                for _ in 0..4096 {
                    rng.fill_bytes(&mut buf);
                    let mut buf = &buf[..];
                    while !buf.is_empty() {
                        let n = future::poll_fn(|cx| writer.as_mut().poll_write(cx, buf))
                            .await
                            .unwrap();
                        buf = &buf[n..];
                    }
                }
                future::poll_fn(|cx| writer.as_mut().poll_flush(cx))
                    .await
                    .unwrap();
            }
        });

        futures::future::try_join(read, write).await.unwrap();
    }
}
