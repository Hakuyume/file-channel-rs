use super::buf::Buf;
use super::runtime::{spawn_blocking, SpawnBlocking};
use std::fs::File;
use std::future::Future;
use std::io::IoSliceMut;
use std::os::fd::AsRawFd;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::{cmp, io};

#[pin_project::pin_project]
#[derive(Clone)]
pub(crate) struct Reader {
    file: Arc<File>,
    offset: i64,
    #[pin]
    state: State,
}

#[pin_project::pin_project(project = StateProj)]
enum State {
    Idle(Option<Buf>),
    Busy(#[pin] SpawnBlocking<(Buf, Option<io::Error>)>),
}

impl Reader {
    pub(crate) fn from_file(file: Arc<File>) -> Self {
        Self {
            file,
            offset: 0,
            state: State::Idle(None),
        }
    }

    pub(crate) fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_vectored(cx, &mut [IoSliceMut::new(buf)])
    }

    pub(crate) fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        loop {
            match this.state.as_mut().project() {
                StateProj::Idle(buf) => {
                    if let Some(buf) = buf {
                        let n = bufs
                            .iter_mut()
                            .map(|b| {
                                let n = cmp::min(b.len(), buf.len());
                                b[..n].copy_from_slice(&buf[..n]);
                                buf.advance(n);
                                n
                            })
                            .sum();
                        if n > 0 {
                            break Poll::Ready(Ok(n));
                        }
                    }

                    let file = this.file.clone();
                    let offset = *this.offset;
                    let mut buf = buf.take().unwrap_or_default();
                    buf.reserve(bufs.iter().map(|buf| buf.len()).sum());
                    let f = spawn_blocking(move || unsafe {
                        let uninit = buf.spare_capacity_mut();
                        let n = libc::pread(
                            file.as_raw_fd(),
                            uninit.as_mut_ptr().cast(),
                            uninit.len(),
                            offset,
                        );
                        if n >= 0 {
                            buf.set_len(buf.len() + n as usize);
                            (buf, None)
                        } else {
                            (buf, Some(io::Error::last_os_error()))
                        }
                    });
                    this.state.set(State::Busy(f));
                }
                StateProj::Busy(f) => {
                    let (buf, e) = match ready!(f.poll(cx)) {
                        Ok((buf, e)) => (Some(buf), e),
                        Err(e) => (None, Some(e)),
                    };
                    let n = buf.as_ref().map_or(0, Buf::len);
                    *this.offset += n as i64;
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
}

impl Clone for State {
    fn clone(&self) -> Self {
        match self {
            Self::Idle(buf) => Self::Idle(buf.clone()),
            Self::Busy(_) => Self::Idle(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    async fn read(reader: &mut Pin<&mut super::Reader>) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        loop {
            let offset = buf.len();
            buf.resize(offset + 4, 0);
            let n =
                futures::future::poll_fn(|cx| reader.as_mut().poll_read(cx, &mut buf[offset..]))
                    .await?;
            buf.truncate(offset + n);
            if n == 0 {
                break Ok(buf);
            }
        }
    }

    #[tokio::test]
    async fn test() {
        let file = Arc::new(tempfile::tempfile().unwrap());
        (&mut &*file).write_all(b"hello").unwrap();
        (&mut &*file).flush().unwrap();

        let mut reader_0 = pin::pin!(super::Reader::from_file(file.clone()));
        let mut reader_1 = pin::pin!(reader_0.clone());
        assert_eq!(read(&mut reader_1).await.unwrap(), b"hello");

        let mut reader_2 = pin::pin!(reader_1.clone());
        assert_eq!(read(&mut reader_1).await.unwrap(), b"");
        assert_eq!(read(&mut reader_2).await.unwrap(), b"");

        let mut reader_3 = pin::pin!(reader_0.clone());
        (&mut &*file).write_all(b" world").unwrap();
        (&mut &*file).flush().unwrap();
        assert_eq!(read(&mut reader_0).await.unwrap(), b"hello world");
        assert_eq!(read(&mut reader_1).await.unwrap(), b" world");
        assert_eq!(read(&mut reader_2).await.unwrap(), b" world");
        assert_eq!(read(&mut reader_3).await.unwrap(), b"hello world");
    }
}
