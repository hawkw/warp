#![deny(warnings)]
use warp::Filter;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    let filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "tracing=info,warp=debug".to_owned());
    tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let hello = warp::path("hello")
        .and(warp::get())
        .map(|| {
            tracing::info!("saying hello...");
            "Hello, World!"
        })
        .with(warp::trace::context("hello"));
    
    let goodbye = warp::path("goodbye")
        .and(warp::get())
        .map(|| {
            tracing::info!("saying goodbye...");
            "So long and thanks for all the fish!"
        })
        .with(warp::trace::context("goodbye"));
    
    let routes = hello.or(goodbye)
        .with(warp::trace::request());

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}
