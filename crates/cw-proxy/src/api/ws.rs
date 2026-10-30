use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};

use crate::state::DaemonState;

pub async fn api_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<DaemonState>,
) -> impl IntoResponse {
    ws.protocols(["cw"])
        .on_upgrade(move |socket| handle_ws_client(socket, state))
}

pub async fn handle_ws_client(mut socket: WebSocket, state: DaemonState) {
    let mut rx = state.ws_tx.subscribe();
    while let Ok(ev) = rx.recv().await {
        if let Ok(txt) = serde_json::to_string(&ev)
            && socket.send(Message::Text(txt)).await.is_err()
        {
            break;
        }
    }
}
