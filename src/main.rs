mod advanced_filter;
mod advanced_metrics;
mod aggregator;
mod config;
mod dynamic_strategy;
mod executor;
mod grpc;
mod momentum_decay;
mod monitor;
mod position;
mod strategy;
mod swqos;
mod types;

use anyhow::Result;
use log::{error, info};
use solana_sdk::signer::Signer;
use std::sync::Arc;
use tokio::sync::mpsc;
use crossbeam_queue::ArrayQueue;  // 🔥 新增: 无锁队列

use aggregator::Aggregator;
use config::Config;
use executor::TransactionBuilder;
use executor::lightspeed_buy::LightSpeedBuyExecutor;
use executor::sol_trade_sell::SolTradeSellExecutor;
use grpc::GrpcClient;
use position::PositionManager;
use strategy::StrategyEngine;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::init();

    info!("🚀 SolSniper - Pump.fun High-Performance Sniper Bot");
    info!("================================================");

    // 加载配置
    let config = Arc::new(Config::from_env()?);
    config.print_summary();

    // 获取钱包
    let keypair = Arc::new(config.get_keypair()?);
    info!("Wallet: {}", keypair.as_ref().pubkey());

    // 创建无锁队列和通道
    // 🔥 优化: 使用 ArrayQueue 替代 mpsc unbounded channel
    let event_queue = Arc::new(ArrayQueue::new(config.event_queue_capacity));
    let (metrics_tx, metrics_rx) = mpsc::channel(1000);  // 缓冲 1000 个指标
    let (signal_tx, signal_rx) = mpsc::channel(100);  // 缓冲 100 个信号

    info!("✅ 无锁队列已创建 (容量: {})", config.event_queue_capacity);

    // 创建组件
    info!("Initializing components...");

    // 1. gRPC 客户端（支持 X-Token 认证）
    let grpc_client = GrpcClient::new(
        config.grpc_endpoint.clone(),
        config.grpc_x_token.clone(),
    );

    // 2. 聚合器（增强版）
    let aggregator = Arc::new(Aggregator::new(config.clone(), metrics_tx));

    // 3. 策略引擎（增强版 - 需要 aggregator 引用）
    let strategy = Arc::new(StrategyEngine::new(
        config.clone(),
        signal_tx,
        aggregator.clone(),
    ));

    // 4. 交易构建器
    let tx_builder = Arc::new(TransactionBuilder::new());

    // 5. LightSpeed 买入执行器
    let lightspeed_buy = Arc::new(LightSpeedBuyExecutor::new(config.clone(), keypair.clone())?);

    // 7. SolTrade 卖出执行器
    let sol_trade_sell = Arc::new(SolTradeSellExecutor::new(config.clone(), keypair.clone())?);

    // 8. 持仓管理器（使用 LightSpeed 买入 + SolTrade 卖出）
    let position_manager = Arc::new(PositionManager::new(
        config.clone(),
        strategy.clone(),
        tx_builder.clone(),
        lightspeed_buy.clone(),
        sol_trade_sell.clone(),
    ));

    info!("✅ All components initialized");

    // 启动各个组件
    info!("Starting components...");

    // 启动 gRPC 订阅（带自动重连和自动恢复）
    let grpc_handle = {
        let grpc_client = grpc_client.clone();
        let event_queue = event_queue.clone();  // 🔥 克隆 Arc<ArrayQueue>
        tokio::spawn(async move {
            loop {
                info!("🚀 启动 gRPC 订阅任务");
                grpc_client.subscribe_with_reconnect(event_queue.clone()).await;
                // subscribe_with_reconnect 内部已经是无限循环，不应该退出
                // 如果退出了说明发生了严重错误
                error!("❌ gRPC 订阅任务异常退出，5秒后重启...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        })
    };

    // 启动聚合器（带自动恢复）
    let aggregator_handle = {
        let aggregator = aggregator.clone();
        let event_queue = event_queue.clone();  // 🔥 克隆 Arc<ArrayQueue>
        tokio::spawn(async move {
            info!("🚀 启动聚合器任务");
            aggregator.start(event_queue).await;
            // 如果 start 退出，说明发生严重错误
            error!("❌ 聚合器任务异常退出");
        })
    };

    // 启动策略引擎（带自动恢复）
    let strategy_handle = {
        let strategy = strategy.clone();
        tokio::spawn(async move {
            info!("🚀 启动策略引擎任务");
            strategy.start(metrics_rx).await;
            // 如果 start 退出，说明发生严重错误
            error!("❌ 策略引擎任务异常退出");
        })
    };

    // 启动持仓管理器（带自动恢复）
    let position_handle = {
        let position_manager = position_manager.clone();
        tokio::spawn(async move {
            info!("🚀 启动持仓管理器任务");
            position_manager.start(signal_rx).await;
            // 如果 start 退出，说明发生严重错误
            error!("❌ 持仓管理器任务异常退出");
        })
    };

    // 启动定期清理任务
    let cleanup_handle = {
        let aggregator = aggregator.clone();
        let cleanup_interval_secs = config.aggregator_cleanup_interval_secs;
        let window_ttl_secs = config.aggregator_window_ttl_secs;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(cleanup_interval_secs));
            loop {
                interval.tick().await;
                aggregator.cleanup_old_windows(window_ttl_secs);
            }
        })
    };

    info!("✅ All components started");
    info!("🎯 Bot is now running. Press Ctrl+C to stop.");

    // 等待 Ctrl+C 信号
    tokio::signal::ctrl_c().await?;

    info!("Shutting down...");

    // 取消所有任务
    grpc_handle.abort();
    aggregator_handle.abort();
    strategy_handle.abort();
    position_handle.abort();
    cleanup_handle.abort();

    info!("Goodbye!");

    Ok(())
}

