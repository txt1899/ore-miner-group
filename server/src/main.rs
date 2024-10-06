use std::{env, process::exit, sync::Arc};

use crate::{manager::ManagerActor, restful::RESTful, worker::WorkerActor};
use actix::{Actor, Addr};
use actix_web::{web, web::Path, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use cfg_if::cfg_if;
use clap::Parser;
use tracing::*;
use tracing_subscriber::{
    fmt::{format, time::ChronoLocal},
    EnvFilter,
};

mod manager;
mod restful;
mod routes;
mod user;
mod worker;

cfg_if! {
    if #[cfg(feature = "build-version")] {
        include!(concat!(env!("OUT_DIR"), "/version.rs"));
    } else {
        pub const VERSION: &str = "unknown";
    }
}

const CURRENT_VERSION: u64 = 2;

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(long, help = "Config the server listen port")]
    port: u16,
}

fn init_log() {
    let format = format::format()
        .with_level(true) // 显示日志级别
        .with_target(false) // 隐藏日志目标
        //.with_thread_ids(true) // 显示线程 ID
        .with_timer(ChronoLocal::new("[%m-%d %H:%M:%S%.3f]".to_string())) // 使用本地时间格式化时间戳
        //.with_thread_names(true) // 显示线程名称
        .compact(); // 使用紧凑的格式
    let env_filter = EnvFilter::from_default_env()
        .add_directive("server=info".parse().unwrap())
        .add_directive("info".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(env_filter).event_format(format).init();
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    init_log();

    info!("VERSION:{}", VERSION);

    let args = Args::parse();

    let manager = ManagerActor::new().start();

    let restful = Arc::new(RESTful { manager: manager.clone() });

    HttpServer::new(move || {
        App::new()
            //.wrap(middleware::Logger::default())
            .app_data(web::Data::new(restful.clone()))
            .app_data(web::Data::new(manager.clone()))
            .route("/worker/{version}/{pubkey}", web::get().to(worker_endpoint))
            .service(routes::login)
            .service(routes::epoch)
            .service(routes::solution)
            .service(routes::difficulty)
    })
    .workers(4)
    .bind(("0.0.0.0", args.port))?
    .run()
    .await
}

async fn worker_endpoint(
    req: HttpRequest,
    stream: web::Payload,
    path: Path<(String, String)>,
    srv: web::Data<Addr<ManagerActor>>,
) -> Result<HttpResponse, Error> {
    let (version, pubkey) = path.into_inner();
    if version.ne(VERSION) {
        Ok(HttpResponse::Forbidden().body("version mismatch"))
    } else {
        ws::start(WorkerActor::new(srv.get_ref().clone(), pubkey), &req, stream)
    }
}
