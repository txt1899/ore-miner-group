use std::{
    collections::HashMap,
    fmt::Debug,
    process::exit,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
        Mutex,
    },
    time::{Duration, Instant},
};

use actix::{prelude::*, Actor, Addr};
use actix_files::{Files, NamedFile};
use actix_web::{
    cookie::Expiration::Session,
    middleware::Logger,
    web,
    App,
    Error,
    HttpRequest,
    HttpResponse,
    HttpServer,
    Responder,
};
use actix_web_actors::ws;
use clap::Parser;
use colored::Colorize;
use master::{
    config::load_config_file,
    lua::LuaScript,
    ore::{utils::ask_confirm, Miner},
    websocket::{jito, mediator, scheduler, server, session},
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use tokio::time::sleep;
use tracing::{debug, error, info};
use tracing_subscriber::fmt::{format, time::ChronoLocal};

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(
        long,
        value_name = "MICROLAMPORTS",
        help = "Price to pay for compute unit. If dynamic fee url is also set, this value will be the max.",
        default_value = "500000",
        global = true
    )]
    priority_fee: Option<u64>,

    #[arg(long, help = "Enable dynamic priority fees", global = true)]
    dynamic_fee: bool,

    #[arg(long, help = "Add jito tip to the miner. Defaults to false", global = true)]
    jito: bool,
}

async fn index() -> impl Responder {
    NamedFile::open_async("../../../static/index.html").await.unwrap()
}

async fn mine_route(
    req: HttpRequest,
    stream: web::Payload,
    srv: web::Data<Addr<server::ServerActor>>,
) -> Result<HttpResponse, Error> {
    ws::start(
        session::SessionActor {
            id: 0,
            heart_beat: Instant::now(),
            server: srv.get_ref().clone(),
        },
        &req,
        stream,
    )
}

async fn get_count(count: web::Data<AtomicUsize>) -> impl Responder {
    let current_count = count.load(Ordering::SeqCst);
    format!("Visitors: {current_count}")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    lib_shared::log::init_log();

    let args = Args::parse();

    let cfg = load_config_file("./config.json").unwrap();

    debug!("{cfg:?}");

    let rpc_client = RpcClient::new_with_commitment(cfg.rpc, CommitmentConfig::confirmed());
    let default_keypair = cfg.keypair_path;
    let fee_payer_path = cfg.fee_payer.unwrap_or(default_keypair.clone());

    let jito_url = cfg
        .jito_url
        .unwrap_or("https://mainnet.block-engine.jito.wtf/api/v1/transactions".to_string());

    let jito_client = RpcClient::new(jito_url);

    let script = LuaScript::new();

    if cfg.script.is_some_and(|v| v) {
        if !ask_confirm("启用动态gas和jito小费，我已经严格测试脚本逻辑. \n是否继续? [Y/n]")
        {
            exit(0);
        }
        script.load_from_file("./script.lua").unwrap();
    }

    let miner = Arc::new(Miner::new(
        Arc::new(rpc_client),
        args.priority_fee,
        Some(default_keypair),
        cfg.dynamic_fee_url,
        args.dynamic_fee,
        Some(fee_payer_path),
        Arc::new(jito_client),
        cfg.buffer_time,
        script,
    ));

    let app_state = Arc::new(AtomicUsize::new(0));

    let mediator = mediator::MediatorActor::default().start();

    // start server actor
    let server = server::ServerActor {
        addr: mediator.clone(),
        sessions: HashMap::new(),
        miner_count: app_state.clone(),
        rng: Default::default(),
    }
    .start();

    let jito = jito::JitoActor {
        enable: args.jito,
        tip: 0,
    }
    .start();

    // start task actor
    let task = scheduler::Scheduler::new(mediator.clone(), jito, miner).start();

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::from(app_state.clone()))
            .app_data(web::Data::new(server.clone()))
            .app_data(web::Data::new(task.clone()))
            .app_data(web::Data::new(mediator.clone()))
            .service(web::resource("/").to(index))
            .route("/count", web::get().to(get_count))
            .route("/ws", web::get().to(mine_route))
            //.service(Files::new("/static", "./static")) // 测试websocket 服务端
            .wrap(Logger::default())
    })
    .workers(2)
    .bind(("0.0.0.0", cfg.port))?
    .run()
    .await
}
