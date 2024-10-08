use std::time::Duration;

use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use shared::interaction::{ClientResponse, ServerResponse, WorkContent};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tokio_tungstenite::{tungstenite, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::*;

pub enum StreamMessage {
    WorkContent(WorkContent),
    Ping(Vec<u8>),
}

pub enum StreamCommand {
    Response(ServerResponse),
    Ping(Vec<u8>),
}

pub fn new_subscribe(
    url: String,
    max_retry: u32,
    mut notify_shutdown: broadcast::Receiver<()>,
) -> (mpsc::Sender<StreamCommand>, mpsc::Receiver<StreamMessage>) {
    let (reader_tx, reader_rx) = mpsc::channel(100);
    let (writer_tx, mut writer_rx) = mpsc::channel(100);
    let mut attempts = 0;
    tokio::spawn(async move {
        'main: loop {
            attempts += 1;

            let stream = match tokio_tungstenite::connect_async(&url).await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    if attempts > max_retry {
                        break 'main;
                    }

                    if let tungstenite::Error::Http(ref val) = err {
                        if val.status() == 403 {
                            if let Some(body) = val.body() {
                                let data = String::from_utf8(body.clone())
                                    .unwrap_or("unknown".to_string());
                                error!("fail to connect to sever: {data}");
                                break 'main;
                            }
                        }
                    }

                    error!("fail to connect to sever: {err:#}");
                    info!("retry...({attempts}/{max_retry})");
                    tokio::select! {
                        _ = notify_shutdown.recv() => {
                            debug!("[stream] async thread shutdown");
                            break
                        },
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    }
                    continue;
                }
            };

            info!("ws connect to the server");

            let (mut write, mut read) = stream.split();

            loop {
                if let Err(err) = tokio::select! {
                     _ = notify_shutdown.recv() => break 'main,
                     res = writer_rx.recv() => {
                        stream_write(res, &mut write).await
                     },
                     res = read.next() => {
                        stream_read(res, &reader_tx).await
                     },
                } {
                    // before shutdown signal, this should never happen.
                    if writer_rx.is_closed() || reader_tx.is_closed() {
                        error!("unrecoverable error: {err:?}");
                        break 'main;
                    } else {
                        error!("{err:?}");
                        break;
                    }
                }
            }
            error!("server disconnected, retries in 10 seconds");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        debug!("[stream] async thread shutdown");
    });

    (writer_tx, reader_rx)
}

type StreamWriter = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
// type StreamReader = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// receive the command and sent to server
async fn stream_write(data: Option<StreamCommand>, ws_tx: &mut StreamWriter) -> anyhow::Result<()> {
    match data {
        None => anyhow::bail!("command channel closed"),
        Some(data) => {
            match data {
                StreamCommand::Response(data) => ws_tx.send(Message::Binary(data.into())).await,
                StreamCommand::Ping(ping) => ws_tx.send(Message::Ping(ping)).await,
            }
            .map_err(|err| anyhow::anyhow!("ws disconnection: {err:?}"))
        }
    }
}

/// read data from stream and use the channel send to stream process
async fn stream_read(
    data: Option<Result<Message, tungstenite::Error>>,
    tx: &mpsc::Sender<StreamMessage>,
) -> anyhow::Result<()> {
    match data {
        None => anyhow::bail!("ws disconnection"),
        Some(Err(err)) => anyhow::bail!(err.to_string()),
        Some(Ok(message)) => {
            match message {
                Message::Binary(bin) => {
                    let data = ClientResponse::from(bin);
                    match data {
                        ClientResponse::MiningWork(work) => {
                            if let Err(_err) = tx.send(StreamMessage::WorkContent(work)).await {
                                anyhow::bail!("message channel closed")
                            }
                        }
                    }
                }
                Message::Ping(ping) => {
                    debug!("ping arrived");
                    if let Err(_err) = tx.send(StreamMessage::Ping(ping)).await {
                        anyhow::bail!("message channel closed")
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
