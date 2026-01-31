//! Server-Sent Events for live updates.

use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// Create an SSE stream from a broadcast channel.
pub fn create_sse_stream(
    rx: tokio::sync::broadcast::Receiver<String>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream =
        BroadcastStream::new(rx).filter_map(|result: Result<String, BroadcastStreamRecvError>| {
            result.ok().map(|data| Ok(Event::default().data(data)))
        });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
