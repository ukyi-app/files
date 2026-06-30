use files::config::Config;
use files::http;
use files::store::reconcile;
use std::future::IntoFuture;
use std::time::Duration;

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).json().try_init();
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cfg = Config::from_env(|k| std::env::var(k).ok()).map_err(|e| anyhow::anyhow!(e))?;
    // 기동 불변식: upload_timeout < gc_grace
    cfg.validate().map_err(|e| anyhow::anyhow!(e))?;

    let internal_port = cfg.internal_port;
    let public_port = cfg.public_port;
    let gc_grace = Duration::from_secs(cfg.gc_grace_secs);
    let reconcile_interval = Duration::from_secs(cfg.gc_grace_secs);
    let data_dir = cfg.data_dir.clone();

    let state = http::build_state(cfg)?;

    // 부팅 시 reconciliation 1회
    if let Err(e) = reconcile::run_once(&data_dir, gc_grace).await {
        tracing::warn!(error = %e, "boot reconciliation failed");
    }

    // 주기 reconciliation(저속 스크럽 포함). grace-period GC라 진행 중 업로드와 안전 공존.
    {
        let dd = data_dir.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(reconcile_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await; // 즉시 발생하는 첫 tick은 boot에서 이미 수행
            loop {
                tick.tick().await;
                match reconcile::run_once(&dd, gc_grace).await {
                    Ok(stats) => tracing::info!(?stats, "reconcile"),
                    Err(e) => tracing::warn!(error = %e, "reconcile failed"),
                }
            }
        });
    }

    let internal = http::internal::router(state.clone());
    let public = http::public::router(state);

    let il = tokio::net::TcpListener::bind(("0.0.0.0", internal_port)).await?;
    let pl = tokio::net::TcpListener::bind(("0.0.0.0", public_port)).await?;
    tracing::info!(internal_port, public_port, "files listening");

    let internal_srv = axum::serve(il, internal).with_graceful_shutdown(shutdown_signal());
    let public_srv = axum::serve(pl, public).with_graceful_shutdown(shutdown_signal());

    let (ri, rp) = tokio::join!(internal_srv.into_future(), public_srv.into_future());
    ri?;
    rp?;
    Ok(())
}
