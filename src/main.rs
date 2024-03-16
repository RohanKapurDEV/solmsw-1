use std::{collections::HashSet, env::var};
use std::{hash::Hash, sync::Arc};

use axum::{extract::State as AxumState, routing::post, Json};
use dotenv::dotenv;
use futures::{stream::futures_unordered::FuturesUnordered, StreamExt};
use teloxide::{dispatching::dialogue::InMemStorage, prelude::*};
use tokio::sync::{mpsc, Mutex};

#[derive(Clone, Debug)]
struct State {
    tx: mpsc::Sender<()>,
    list: Arc<Mutex<HashSet<ChatId>>>, // this is an extremely stupid design choice but idrc lmao
}

#[derive(Clone, Default)]
enum ChatStatus {
    #[default]
    Start,
}

type MyDialogue = Dialogue<ChatStatus, InMemStorage<ChatStatus>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() {
    dotenv().ok();
    let mut futs = FuturesUnordered::new();

    let (tx, mut rx) = mpsc::channel::<()>(1000);
    let list = Arc::new(Mutex::new(HashSet::new()));

    let list_handle = list.clone();

    let api_state = Arc::new(State { tx, list });
    let api_state_2 = api_state.clone();

    let teloxide_token = var("TELOXIDE_TOKEN").expect("TELOXIDE_TOKEN is not set");
    let bot = Bot::new(teloxide_token);

    let api = axum::Router::new()
        .route("/receive_event", post(receive_event))
        .with_state(api_state);

    // Handle commands
    let y = bot.clone();
    futs.push(tokio::spawn(async move {
        Dispatcher::builder(
            y,
            Update::filter_message()
                .enter_dialogue::<Message, InMemStorage<ChatStatus>, ChatStatus>()
                .branch(dptree::case![ChatStatus::Start].endpoint(register_chat_id)),
        )
        .dependencies(dptree::deps![
            InMemStorage::<ChatStatus>::new(),
            api_state_2.clone()
        ])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    }));

    // API server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    futs.push(tokio::spawn(async move {
        axum::serve(listener, api).await.unwrap();
    }));

    // Channel responder - pushes messages to telegram
    let x = bot.clone();
    futs.push(tokio::spawn(async move {
        while let Some(_) = rx.recv().await {
            let list = list_handle.lock().await;
            for chat_id in list.iter() {
                x.send_message(*chat_id, "New multisig event detected")
                    .send()
                    .await
                    .unwrap();
            }
        }
    }));

    // Wait for all futures to complete
    while let Some(fut) = futs.next().await {
        fut.unwrap();
    }
    println!("All futures completed");
}

async fn receive_event(
    AxumState(state): AxumState<Arc<State>>,
    Json(_body): Json<serde_json::Value>,
) -> Json<()> {
    let tx = state.tx.clone();
    let chan_send = tx.send(()).await;

    if chan_send.is_err() {
        println!("Failed to send event to the bot");
    }

    Json(())
}

async fn register_chat_id(
    bot: Bot,
    _dialogue: MyDialogue,
    message: Message,
    state: Arc<State>,
) -> HandlerResult {
    let chat_id = message.chat.id;

    let mut list = state.list.lock().await;

    if list.contains(&chat_id) {
        let try_msg = bot
            .send_message(
                chat_id,
                "You are already registered for multisig events relax bro",
            )
            .send()
            .await;

        if try_msg.is_err() {
            println!("Failed to send already in message to chat_id: {}", chat_id);
        }

        return Ok(());
    }

    list.insert(chat_id);

    let try_msg = bot
        .send_message(chat_id, "You have been registered for multisig events")
        .send()
        .await;

    if try_msg.is_err() {
        println!("Failed to send message to chat_id: {}", chat_id);
    }

    Ok(())
}
