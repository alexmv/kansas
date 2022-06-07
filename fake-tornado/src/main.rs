use actix_web::middleware::Logger;
use actix_web::{
    delete, get, http::header::ContentType, post, web, App, HttpResponse, HttpServer, Responder,
};
use clap::{Arg, Command};
use env_logger::Env;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

struct AppState {
    sleep_duration: Duration,
    heartbeat_id: Mutex<u32>,
    next_queue_id: Mutex<u32>,
}

#[derive(Deserialize)]
struct QueueIdForm {
    queue_id: Option<String>,
}

#[derive(Deserialize)]
struct GetEventsForm {
    queue_id: String,
    dont_block: String,
}

#[post("/events/internal")]
async fn create_queue(form: web::Form<QueueIdForm>, data: web::Data<AppState>) -> impl Responder {
    let queue_id = match form.queue_id.clone() {
        Some(provided) => provided,
        None => {
            let mut queue_int = data.next_queue_id.lock();
            *queue_int += 1;
            format!("{queue_int}:1")
        }
    };
    let resp = json!({"result":"success","msg":"","events":[], "queue_id": queue_id});
    HttpResponse::Ok()
        .append_header(("x-tornado-queue-id", queue_id))
        .content_type(ContentType::json())
        .body(resp.to_string())
}

#[delete("/events")]
async fn delete_queue(form: web::Form<QueueIdForm>) -> impl Responder {
    let queue_id = form.queue_id.clone().expect("No queue-id");
    let resp = json!({"result":"success","msg":""});
    HttpResponse::Ok()
        .content_type(ContentType::json())
        .append_header(("x-tornado-queue-id", queue_id))
        .body(resp.to_string())
}

#[get("/events")]
async fn poll_events(
    parameters: web::Query<GetEventsForm>,
    data: web::Data<AppState>,
) -> impl Responder {
    let queue_id = parameters.queue_id.clone();
    if parameters.dont_block == "false" {
        tokio::time::sleep(data.sleep_duration).await;
    }
    let mut heartbeat_id = data.heartbeat_id.lock();
    *heartbeat_id += 1;
    let resp = json!({
        "result":"success",
        "msg":"",
        "events":[{"type":"heartbeat","id":*heartbeat_id}],
        "queue_id":queue_id,
    });

    HttpResponse::Ok()
        .content_type(ContentType::json())
        .body(resp.to_string())
}

#[actix_web::main] // or #[tokio::main]
async fn main() -> std::io::Result<()> {
    let matches = Command::new("Kansas")
        .version("1.0")
        .about("Pretend Tornado server, for testing")
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("Listen port")
                .required(true)
                .takes_value(true)
                .forbid_empty_values(true),
        )
        .arg(
            Arg::new("time")
                .short('t')
                .long("time")
                .value_name("SECONDS")
                .help("Seconds to sleep")
                .takes_value(true)
                .forbid_empty_values(true)
                .default_value("50"),
        )
        .get_matches();
    let port = matches.value_of("port").unwrap();
    let port = port
        .parse()
        .unwrap_or_else(|_| panic!("Unable to parse port: {}", port));

    let sleep_duration = Duration::from_secs(
        matches
            .value_of("time")
            .expect("No time given")
            .parse::<u64>()
            .unwrap(),
    );
    let state = web::Data::new(AppState {
        sleep_duration,
        heartbeat_id: Mutex::new(0),
        next_queue_id: Mutex::new(0),
    });

    env_logger::init_from_env(Env::default().default_filter_or("info"));
    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(state.clone())
            .route("/health", web::get().to(|| async { "OK!" }))
            .service(
                web::scope("/json")
                    .service(create_queue)
                    .service(delete_queue)
                    .service(poll_events),
            )
            .service(
                web::scope("/api/v1")
                    .service(create_queue)
                    .service(delete_queue)
                    .service(poll_events),
            )
    })
    .backlog(8192)
    .max_connection_rate(1000)
    .bind(("127.0.0.1", port))?
    .run()
    .await
}
