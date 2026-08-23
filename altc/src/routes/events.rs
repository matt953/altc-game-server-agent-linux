use crate::db::{devices, users::User};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::{Stream, StreamExt};
use std::collections::HashSet;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;

pub async fn stream(
    State(s): State<AppState>,
    user: User,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let mine: HashSet<String> = devices::client_ids_for_user(&s.pool, user.id)
        .await?
        .into_iter()
        .collect();
    let is_admin = user.is_admin();

    let stream = BroadcastStream::new(s.events.subscribe()).filter_map(move |ev| {
        let visible = ev
            .as_ref()
            .ok()
            .filter(|e| is_admin || mine.contains(&e.client_id))
            .and_then(|e| Event::default().json_data(e).ok());
        std::future::ready(visible.map(Ok))
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
