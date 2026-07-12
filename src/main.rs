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
    // GC 무덤 정산 대기의 **유일한 상계**. `upload_timeout`에서 파생한다(새 env 노브 없음).
    // cfg가 build_state로 move되기 **전에** 뽑는다(gc_grace와 동형). 기본값: 600s + 60s = 660s.
    let settle_timeout = reconcile::settle_timeout_from(Duration::from_secs(cfg.upload_timeout_secs));

    let state = http::build_state(cfg)?; // ← 여기서 cfg가 move된다(그리고 .objects를 만든다)

    // 부팅 시 reconciliation 1회. ⚠ **put과 같은 Store**를 넘긴다(D-1/D-3) — 핀 등록부는
    // in-process이고, 같은 root로 Store를 새로 만들면 등록부가 갈라져 버그가 부활한다.
    if let Err(e) = reconcile::run_once(&state.store, gc_grace, settle_timeout).await {
        tracing::warn!(error = %e, "boot reconciliation failed");
    }

    // 주기 reconciliation(저속 스크럽 포함). grace-period GC라 진행 중 업로드와 안전 공존.
    {
        let s = state.store.clone(); // 같은 Arc 등록부를 공유(Store::clone)
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(reconcile_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await; // 즉시 발생하는 첫 tick은 boot에서 이미 수행
            loop {
                tick.tick().await;
                match reconcile::run_once(&s, gc_grace, settle_timeout).await {
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
