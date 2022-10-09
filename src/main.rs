use axum::extract::Path;
use clap::Parser;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use adsb_deku::deku::prelude::*;
use adsb_deku::{Frame, ICAO};
use rsadsb_common::Airplanes;

use axum::{response::IntoResponse, routing::get, Json, Router};
use std::net::SocketAddr;

/// Rust ADS-B processor and web server providing information in json format
#[derive(Parser, Debug, Clone, Copy)]
pub struct Args {
    lat: f64,
    long: f64,
    #[arg(short, long)]
    serve_addr: SocketAddr,

    #[arg(short, long)]
    dump1090_addr: SocketAddr,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let adsb_airplanes = Arc::new(Mutex::new(Airplanes::new()));

    // initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "spotter=info,adsb_deku=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // build our application with a route
    let app = Router::new()
        .route(
            "/",
            get({
                let a = Arc::clone(&adsb_airplanes);
                move |_: ()| home(a, args)
            }),
        )
        .route(
            "/airplanes",
            get({
                let a = Arc::clone(&adsb_airplanes);
                move |_: ()| airplanes_all(a)
            }),
        )
        .route(
            "/airplane/closest",
            get({
                let a = Arc::clone(&adsb_airplanes);
                move |_: ()| closest_airplane(a)
            }),
        )
        .route(
            "/airplane/furthest",
            get({
                let a = Arc::clone(&adsb_airplanes);
                move |_: ()| furthest_airplane(a)
            }),
        )
        .route(
            "/airplane/:icao",
            get({
                let a = Arc::clone(&adsb_airplanes);
                move |icao: Path<String>| airplane_icao(icao, a)
            }),
        );

    tracing::info!("listening on {}", args.serve_addr);
    tokio::spawn(axum::Server::bind(&args.serve_addr).serve(app.into_make_service()));

    let stream = TcpStream::connect(args.dump1090_addr).await.unwrap();
    tracing::info!("connected to {stream:?}");
    let mut stream = BufReader::new(stream);
    let mut input = String::new();
    loop {
        input.clear();
        if let Ok(len) = stream.read_line(&mut input).await {
            if len == 0 {
                continue;
            }
            // convert from string hex -> bytes
            let hex = &mut input.to_string()[1..len - 2].to_string();
            tracing::debug!("{}", hex.to_lowercase());
            let bytes = if let Ok(bytes) = hex::decode(&hex) {
                bytes
            } else {
                continue;
            };

            // check for all 0's
            if bytes.iter().all(|&b| b == 0) {
                continue;
            }

            // decode
            if let Ok((_, frame)) = Frame::from_bytes((&bytes, 0)) {
                let mut a = adsb_airplanes.lock().await;
                a.action(frame, (args.lat, args.long), 500.0);

                // remove airplanes that timed-out after 2 minutes
                a.prune(120);
            }
        }
    }
}

// reply back with all airplanes
async fn home(adsb_airplanes: Arc<Mutex<Airplanes>>, args: Args) -> impl IntoResponse {
    tracing::info!("home");
    let a = adsb_airplanes.lock().await;
    let body = format!(
        r#"Spotter - Rust ADS-B processor and web server providing information in json format

==[Info]==========
Lat: {}
Long: {}
Airplanes tracked: {}

==[Protocol]=====
/airplanes
/airplanes/closest
/airplanes/furthest
/airplanes/:icao
"#,
        args.lat,
        args.long,
        a.len()
    );

    body
}

// reply back with all airplanes
async fn airplanes_all(adsb_airplanes: Arc<Mutex<Airplanes>>) -> impl IntoResponse {
    tracing::info!("airplanes");
    let a = adsb_airplanes.lock().await;
    Json(a.clone())
}

async fn closest_airplane(adsb_airplanes: Arc<Mutex<Airplanes>>) -> impl IntoResponse {
    tracing::info!("closest");
    let a = adsb_airplanes.lock().await;
    let mut minimum = None;
    for (icao, airplane_state) in a.iter() {
        if let Some(kilo_distance) = airplane_state.coords.kilo_distance {
            if minimum.is_none() {
                minimum = Some((icao.clone().to_string(), airplane_state.clone()));
            }
            if let Some(ref inner_minimum) = minimum {
                if Some(kilo_distance) < inner_minimum.1.coords.kilo_distance {
                    minimum = Some((icao.clone().to_string(), airplane_state.clone()));
                }
            }
        }
    }
    Json(minimum)
}

async fn furthest_airplane(adsb_airplanes: Arc<Mutex<Airplanes>>) -> impl IntoResponse {
    tracing::info!("furthest");
    let a = adsb_airplanes.lock().await;
    let mut minimum = None;
    for (icao, airplane_state) in a.iter() {
        if let Some(kilo_distance) = airplane_state.coords.kilo_distance {
            if minimum.is_none() {
                minimum = Some((icao.clone().to_string(), airplane_state.clone()));
            }
            if let Some(ref inner_minimum) = minimum {
                if Some(kilo_distance) > inner_minimum.1.coords.kilo_distance {
                    minimum = Some((icao.clone().to_string(), airplane_state.clone()));
                }
            }
        }
    }
    Json(minimum)
}

async fn airplane_icao(
    Path(icao): Path<String>,
    adsb_airplanes: Arc<Mutex<Airplanes>>,
) -> impl IntoResponse {
    tracing::info!(icao);
    let a = adsb_airplanes.lock().await;

    let details = a.aircraft_details(ICAO::from_str(&icao).unwrap());
    Json(details)
}
