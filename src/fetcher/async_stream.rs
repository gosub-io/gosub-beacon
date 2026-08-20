use bytes::Bytes;
use futures_core::Stream as FStream;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::{cmp, mem};

/// Default buffer size of our vec<u8> when we cannot determine the size of our stream. Seems a reasonable default.
const DEFAULT_BUF_SIZE: usize = 1024;

// This defines that a stream is a future_core stream that returns a result or Bytes. This is how we get
// our data from the underlying library (reqwest in this case), but we want to work with vec<u8>.
// type Stream = dyn FStream<Item=anyhow::Result<Bytes>>;
type Stream = Pin<Box<dyn FStream<Item = anyhow::Result<Bytes>> + Send>>;

/// This wrapper merely converts the Error type (E) from Future_core stream to an anyhow::Error. This
/// is the error type that we use for our purposes.
pub struct AsyncStreamWrap<S: FStream<Item = Result<Bytes, E>> + Send, E: Error + Send + Sync + 'static>(S);

impl<S, E> FStream for AsyncStreamWrap<S, E>
where
    S: FStream<Item = Result<Bytes, E>> + Send,
    E: Error + Send + Sync + 'static,
{
    type Item = anyhow::Result<Bytes>;

    /// Poll next() is called to fetch the new poll. It will return either a Poll::Pending when there is no data (yet),
    /// Poll::Ready() when there is data (or when the stream has ended). We just pass the data through, except when
    /// there is an error. In that case, we convert that error into an anyhow::error, which is what we want our errors to be.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = unsafe { self.map_unchecked_mut(|this| &mut this.0) };

        match stream.poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(bytes))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(anyhow::Error::new(e)))),
            Poll::Ready(None) => Poll::Ready(None),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

/// Allows to clone the async stream wrapper
impl<S, E> Clone for AsyncStreamWrap<S, E>
where
    S: FStream<Item = Result<Bytes, E>> + Clone + Send,
    E: Error + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        AsyncStreamWrap(self.0.clone())
    }
}

/// Defines an asynchronous stream.
pub struct AsyncStream {
    stream: Pin<Box<dyn FStream<Item = anyhow::Result<Bytes>> + Send>>,
    /// Optional length given to the stream. Can be None when the stream length is not known (beforehand)
    len: Option<usize>,
}

impl AsyncStream {
    /// Create a new Async stream.
    pub fn new<E, S>(stream: S, len: Option<usize>) -> Self
    where
        E: Error + Send + Sync + 'static,
        S: FStream<Item = Result<Bytes, E>> + Send + 'static,
    {
        Self {
            stream: Box::pin(AsyncStreamWrap(stream)),
            len,
        }
    }

    /// Method to convert the stream (of bytes::Bytes) into a vec<u8>.
    pub fn vec(self) -> ToVec {
        // We try to determine the size of the stream (if possible), or use a default size if we can't
        // detect this. This will set the capacity of our destination buffer.
        let (min_size, size) = self.stream.size_hint();
        let size = size.or(self.len).unwrap_or(cmp::max(DEFAULT_BUF_SIZE, min_size));

        ToVec {
            source_stream: self.stream,
            dest_buf: Vec::with_capacity(size),
        }
    }

    /// This function captures a whole stream and turns it into a Bytes::bytes
    pub fn bytes(self) -> impl Future<Output = anyhow::Result<Bytes>> {
        let to_vec = self.vec();
        async move {
            let vec = to_vec.await?;
            Ok(Bytes::from(vec))
        }
    }
}

/// This struct allows to return a stream of Bytes into a vec<u8>
pub struct ToVec {
    /// Actual stream that will be converted
    source_stream: Stream,
    /// Destination buffer to store all the u8's we receive from the inner stream
    dest_buf: Vec<u8>, //TODO: this should be bytes and then in the end it can be copied to a single Vec<u8>
}

impl Future for ToVec {
    /// This future will output a Result<Vec<u8>>, or Result<None> when the stream is finished
    type Output = anyhow::Result<Vec<u8>>;

    /// The poll() function will poll data from the inner stream (with bytes) and converts that data
    /// to a vec<u8>.
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let source_stream = self.source_stream.as_mut();
            let chunk = ready!(source_stream.poll_next(cx));

            match chunk {
                // A chunk of data will be converted to vec<u8> and added to our destination buffer
                Some(Ok(chunk)) => self.dest_buf.extend_from_slice(&chunk),
                // An error has occurred, so we return an Ready with error
                Some(Err(e)) => return Poll::Ready(Err(e)),
                // No data found, so the stream is ready. We take our destination buffer anfd give it back to the caller
                None => return Poll::Ready(Ok(mem::take(&mut self.dest_buf))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Builder;

    #[test]
    fn test_async_stream() {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();

        rt.block_on(async move {
            // Local one-shot server: the test used to stream from httpbin.org, which
            // made it fail whenever that service had a bad day.
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = vec![0xAB_u8; 100000];
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            });

            let stream = reqwest::get(format!("http://127.0.0.1:{port}/")).await.unwrap().bytes_stream();

            let bytes = AsyncStream::new(stream, None).bytes().await.unwrap();

            assert_eq!(bytes.len(), 100000);
            assert!(bytes.iter().all(|b| *b == 0xAB));
        });
    }
}
