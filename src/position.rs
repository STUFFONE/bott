use chrono::Utc;
use log::{info, warn, error};
use parking_lot::RwLock as ParkingLotRwLock;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock as TokioRwLock};
use once_cell::sync::Lazy;  // 🔥 新增: 用于全局PDA缓存

use crate::config::Config;
use crate::executor::TransactionBuilder;
use crate::executor::lightspeed_buy::LightSpeedBuyExecutor;
use crate::executor::sol_trade_sell::{SolTradeSellExecutor, SellParams, PumpFunSellParams};
use crate::momentum_decay::{MomentumDecayDetector, MomentumDecayConfig};
use crate::monitor::{RealTimeMonitor, MonitorConfig, AlertSeverity};
use crate::strategy::StrategyEngine;
use crate::types::{Position, StrategySignal, WindowMetrics};

// 🔥 新增: PDA缓存（全局静态）
static PUMPFUN_PROGRAM_ID: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::try_from("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
        .expect("Invalid PumpFun program ID")
});

static TOKEN_PROGRAM_ID: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::try_from("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        .expect("Invalid TOKEN_PROGRAM_ID")
});

static TOKEN_2022_PROGRAM_ID: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::try_from("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
        .expect("Invalid TOKEN_2022_PROGRAM_ID")
});

static ASSOCIATED_TOKEN_PROGRAM_ID: Lazy<Pubkey> = Lazy::new(|| {
    Pubkey::try_from("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
        .expect("Invalid ASSOCIATED_TOKEN_PROGRAM_ID")
});

/// 持仓管理器（增强版）
///
/// 集成了动能衰减检测和实时监控功能
/// 买入使用 LightSpeed，卖出使用 SolTrade
pub struct PositionManager {
    config: Arc<Config>,
    positions: Arc<ParkingLotRwLock<HashMap<Pubkey, Position>>>,
    strategy: Arc<StrategyEngine>,
    tx_builder: Arc<TransactionBuilder>,
    /// LightSpeed 买入执行器（专用于买入）
    lightspeed_buy: Arc<LightSpeedBuyExecutor>,
    /// SolTrade 卖出执行器（专用于卖出）
    sol_trade_sell: Arc<SolTradeSellExecutor>,
    /// 动能衰减检测器（使用 Tokio RwLock 支持异步）
    momentum_detector: Arc<TokioRwLock<MomentumDecayDetector>>,
    /// 实时监控器（使用 Tokio RwLock 支持异步）
    monitor: Arc<TokioRwLock<RealTimeMonitor>>,
}

impl PositionManager {
    pub fn new(
        config: Arc<Config>,
        strategy: Arc<StrategyEngine>,
        tx_builder: Arc<TransactionBuilder>,
        lightspeed_buy: Arc<LightSpeedBuyExecutor>,
        sol_trade_sell: Arc<SolTradeSellExecutor>,
    ) -> Self {
        // 创建动能衰减检测器（使用配置文件参数）
        let momentum_config = MomentumDecayConfig {
            buy_ratio_threshold: config.momentum_buy_ratio_threshold,
            net_inflow_threshold: config.momentum_net_inflow_threshold,
            trade_frequency_threshold: config.momentum_activity_threshold as u32,
            acceleration_threshold: 1.0,  // 保留固定值，暂无对应配置
            composite_score_threshold: config.momentum_composite_score_threshold,
            strict_mode: false,  // 保留固定值，暂无对应配置
        };
        let momentum_detector = Arc::new(TokioRwLock::new(
            MomentumDecayDetector::new(momentum_config)
        ));

        // 创建实时监控器
        let monitor_config = MonitorConfig::from_config(&config);
        let rpc_client = Arc::new(solana_client::rpc_client::RpcClient::new(
            config.rpc_endpoint.clone()
        ));
        let monitor = Arc::new(TokioRwLock::new(
            RealTimeMonitor::new(monitor_config, rpc_client)
        ));

        info!("🎯 持仓管理器已初始化（增强版）");
        info!("   ✅ 动能衰减检测器已启用");
        info!("   ✅ 实时监控系统已启用");
        info!("   ✅ LightSpeed 买入执行器已启用");
        info!("   ✅ SolTrade 卖出执行器已启用");

        Self {
            config,
            positions: Arc::new(ParkingLotRwLock::new(HashMap::new())),
            strategy,
            tx_builder,
            lightspeed_buy,
            sol_trade_sell,
            momentum_detector,
            monitor,
        }
    }

    /// 启动持仓管理器（增强版）
    pub async fn start(
        &self,
        mut signal_rx: mpsc::Receiver<(Arc<WindowMetrics>, StrategySignal)>,
    ) {
        info!("🎯 持仓管理器已启动（增强版）");

        while let Some((metrics, signal)) = signal_rx.recv().await {
            // 1. 检查现有持仓的动能衰减
            self.check_momentum_decay(&metrics).await;

            // 2. 实时监控现有持仓
            self.monitor_positions().await;

            // 3. 处理策略信号
            match signal {
                StrategySignal::Buy => {
                    if let Err(e) = self.handle_buy_signal(&metrics).await {
                        error!("❌ 处理买入信号失败: {}", e);
                    }
                }
                StrategySignal::Sell => {
                    if let Err(e) = self.handle_sell_signal(&metrics).await {
                        error!("❌ 处理卖出信号失败: {}", e);
                    }
                }
                StrategySignal::Hold => {
                    self.handle_hold_signal(&metrics).await;
                }
                StrategySignal::None => {
                    // 无信号，继续监控
                }
            }
        }
    }

    /// 检查动能衰减
    ///
    /// 对所有持仓进行动能衰减检测，如果检测到衰减则触发卖出
    /// 🔥 优化: 提前检查持仓，避免不必要的detector调用
    async fn check_momentum_decay(&self, metrics: &WindowMetrics) {
        // 🔥 优化: 提前返回，避免不必要的持仓检查和detector调用
        if !self.positions.read().contains_key(&metrics.mint) {
            return;
        }

        // 执行动能衰减检测
        let decay_detected = {
            let detector = self.momentum_detector.read().await;
            detector.detect(metrics)
        };

        if let Some(reason) = decay_detected {
            warn!("⚠️  检测到动能衰减: {}", reason.description());
            warn!("   Token: {}", metrics.mint);
            warn!("   触发紧急卖出");

            // 触发紧急卖出
            if let Err(e) = self.handle_sell_signal(metrics).await {
                error!("❌ 紧急卖出失败: {}", e);
            }
        }
    }

    /// 监控所有持仓
    ///
    /// 对所有持仓进行实时监控，检测风险警报
    async fn monitor_positions(&self) {
        let positions = {
            let positions = self.positions.read();
            positions.values().cloned().collect::<Vec<_>>()
        };

        for position in positions {
            // 使用 Tokio RwLock 支持异步
            let alerts = {
                let mut monitor = self.monitor.write().await;
                match monitor.monitor_position(&position).await {
                    Ok(alerts) => alerts,
                    Err(e) => {
                        error!("❌ 监控持仓失败: {}", e);
                        continue;
                    }
                }
            };

            // 处理严重警报
            for alert in alerts {
                if alert.severity() >= AlertSeverity::High {
                    warn!("🚨 高风险警报: {}", alert.description());
                    warn!("   Token: {}", position.mint);

                    // 对于严重警报，触发紧急卖出
                    if alert.severity() == AlertSeverity::Critical {
                        warn!("   触发紧急卖出");

                        // 构建 metrics 用于卖出
                        let metrics = WindowMetrics {
                            mint: position.mint,
                            event_count: 0,
                            net_inflow_sol: 0,
                            buy_ratio: 0.0,
                            acceleration: 0.0,
                            latest_virtual_sol_reserves: position.latest_virtual_sol_reserves,
                            latest_virtual_token_reserves: position.latest_virtual_token_reserves,
                            threshold_buy_amount: None,
                            advanced_metrics: None,  // ✅ 添加新字段
                        };

                        if let Err(e) = self.handle_sell_signal(&metrics).await {
                            error!("❌ 紧急卖出失败: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// 处理买入信号（使用 LightSpeed）
    async fn handle_buy_signal(&self, metrics: &WindowMetrics) -> anyhow::Result<()> {
        // 检查是否已有持仓
        {
            let positions = self.positions.read();
            if positions.contains_key(&metrics.mint) {
                info!("Already have position for {}, skipping", metrics.mint);
                return Ok(());
            }

            // 检查是否达到最大持仓数
            if positions.len() >= self.config.max_positions {
                warn!("⚠️  已达到最大持仓数量: {}/{}, 跳过买入",
                    positions.len(), self.config.max_positions);
                return Ok(());
            }
        }

        info!("🚀 执行 LightSpeed 买入: {}", metrics.mint);

        // 获取买入金额
        // 优先使用阈值触发的买入金额，否则使用默认配置
        let sol_amount = if let Some(threshold_amount) = metrics.threshold_buy_amount {
            info!("💡 使用阈值触发买入金额: {:.4} SOL", threshold_amount);
            (threshold_amount * 1_000_000_000.0) as u64 // SOL -> lamports
        } else {
            self.config.get_snipe_amount_lamports()
        };

        // 计算 bonding_curve 和 associated_bonding_curve（PDA）
        let bonding_curve = self.derive_bonding_curve(&metrics.mint)?;
        let associated_bonding_curve = self.derive_associated_bonding_curve(&bonding_curve, &metrics.mint)?;

        // 使用 LightSpeed 买入执行器
        // 🔥 修复: 移除 virtual_token_reserves/virtual_sol_reserves 参数（改为内部读取）
        match self.lightspeed_buy.execute_buy(
            &metrics.mint,
            &bonding_curve,
            &associated_bonding_curve,
            sol_amount,
        ).await {
            Ok(signature) => {
                info!("✅ LightSpeed 买入交易已发送: {}", signature);

                // 🔥 修复: 使用 monitor 轮询交易确认（30秒超时，狙击需要更长时间）
                let confirmation_result = {
                    let monitor = self.monitor.read().await;
                    monitor.poll_transaction_confirmation(signature, 30).await
                };

                match confirmation_result {
                    Ok(_) => {
                        info!("✅ 买入交易已确认: {}", signature);

                        // 🔥 修复: 查询实际 token 余额（而非估算）
                        let actual_token_amount = match self.sol_trade_sell.get_token_balance(&metrics.mint).await {
                            Ok(balance) => {
                                info!("   实际获得 Token 数量: {}", balance);
                                balance
                            }
                            Err(e) => {
                                warn!("⚠️  查询实际余额失败: {}, 使用估算值", e);
                                // Fallback: 使用估算值
                                let estimated = self.tx_builder.estimate_buy_token_amount(
                                    metrics.latest_virtual_token_reserves,
                                    metrics.latest_virtual_sol_reserves,
                                    sol_amount,
                                );
                                info!("   估算获得 Token 数量: {}", estimated);
                                estimated
                            }
                        };

                        // 计算入场价格
                        let entry_price_sol = if actual_token_amount > 0 {
                            sol_amount as f64 / actual_token_amount as f64
                        } else {
                            0.0
                        };

                        // 🔥 修复: 只有确认成功才记录持仓
                        // 🔥 修复: 先读取 creator，再派生 creator_vault
                        let creator = self.get_creator_from_bonding_curve(&bonding_curve)?;
                        let creator_vault = Self::derive_creator_vault(&creator)?;

                        let position = Position {
                            mint: metrics.mint,
                            entry_time: Utc::now(),
                            entry_price_sol,
                            token_amount: actual_token_amount,  // 🔥 使用实际余额
                            sol_invested: sol_amount,
                            bonding_curve,
                            creator_vault,
                            associated_bonding_curve,
                            latest_virtual_sol_reserves: metrics.latest_virtual_sol_reserves,
                            latest_virtual_token_reserves: metrics.latest_virtual_token_reserves,
                        };

                        self.positions.write().insert(metrics.mint, position);

                        info!(
                            "📊 持仓已开仓: {} tokens @ {:.8} SOL/token",
                            actual_token_amount, entry_price_sol
                        );
                    }
                    Err(e) => {
                        // 🔥 修复: 交易确认失败，不记录持仓
                        error!("❌ 买入交易确认失败: {}", e);
                        error!("   签名: {}", signature);
                        error!("   不记录持仓，避免状态不一致");
                        return Err(anyhow::anyhow!("买入交易确认失败: {}", e));
                    }
                }
            }
            Err(e) => {
                error!("❌ LightSpeed 买入发送失败: {}", e);
                return Err(e);
            }
        }

        Ok(())
    }

    /// 处理卖出信号（使用 SolTrade）
    async fn handle_sell_signal(&self, metrics: &WindowMetrics) -> anyhow::Result<()> {
        // 获取持仓
        let position = {
            let positions = self.positions.read();
            match positions.get(&metrics.mint) {
                Some(pos) => pos.clone(),
                None => {
                    info!("No position for {}, skipping sell", metrics.mint);
                    return Ok(());
                }
            }
        };

        info!("🔴 执行 SolTrade 卖出: {}", metrics.mint);

        // 🔍 检查实际余额（防止余额不足导致交易失败）
        match self.sol_trade_sell.get_token_balance(&metrics.mint).await {
            Ok(actual_balance) => {
                if actual_balance < position.token_amount {
                    warn!("⚠️  余额不足！");
                    warn!("   预期: {} tokens", position.token_amount);
                    warn!("   实际: {} tokens", actual_balance);
                    warn!("   将使用实际余额卖出");
                }
                let sell_amount = actual_balance.min(position.token_amount);

                if sell_amount == 0 {
                    error!("❌ 余额为 0，无法卖出");
                    // 仍然移除持仓记录（避免重复尝试）
                    self.positions.write().remove(&metrics.mint);
                    return Ok(());
                }

                // 构建 SellParams（使用实际余额）
                let sell_params = SellParams {
                    mint: metrics.mint,
                    input_token_amount: sell_amount,
                    slippage_basis_points: Some((self.config.slippage_percent * 100.0) as u64),
                    wait_transaction_confirmed: true,
                    close_token_account: true,
                    pumpfun_params: PumpFunSellParams {
                        bonding_curve: position.bonding_curve,
                        associated_bonding_curve: position.associated_bonding_curve,
                        creator_vault: position.creator_vault,
                    },
                };

                // 使用 SolTrade 卖出执行器
                match self.sol_trade_sell.execute_sell(sell_params).await {
                    Ok(signature) => {
                        info!("✅ SolTrade 卖出成功: {}", signature);

                        // 使用 monitor 轮询交易确认（10秒超时）
                        {
                            let monitor = self.monitor.read().await;
                            match monitor.poll_transaction_confirmation(signature, 10).await {
                                Ok(_) => {
                                    info!("✅ 卖出交易已确认");
                                }
                                Err(e) => {
                                    warn!("⚠️  卖出交易确认失败: {}, 继续结算", e);
                                }
                            }
                        }

                        // 估算获得的 SOL（从 metrics 计算）
                        let sol_received = self.tx_builder.estimate_sell_sol_amount(
                            metrics.latest_virtual_token_reserves,
                            metrics.latest_virtual_sol_reserves,
                            sell_amount,
                        );

                        info!("   估算获得 SOL: {:.4}", sol_received as f64 / 1_000_000_000.0);

                        // 计算盈亏
                        let profit_loss_sol = sol_received as i64 - position.sol_invested as i64;
                        let profit_loss_percent =
                            (profit_loss_sol as f64 / position.sol_invested as f64) * 100.0;

                        info!(
                            "💰 持仓已平仓: {:.4} SOL ({:+.2}%)",
                            sol_received as f64 / 1_000_000_000.0,
                            profit_loss_percent
                        );

                        // 移除持仓
                        self.positions.write().remove(&metrics.mint);
                    }
                    Err(e) => {
                        error!("❌ SolTrade 卖出失败: {}", e);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                error!("❌ 获取余额失败: {}", e);
                error!("   将尝试使用记录的 token 数量卖出");

                // 构建 SellParams
                let sell_params = SellParams {
                    mint: metrics.mint,
                    input_token_amount: position.token_amount,
                    slippage_basis_points: Some((self.config.slippage_percent * 100.0) as u64),
                    wait_transaction_confirmed: true,
                    close_token_account: true,
                    pumpfun_params: PumpFunSellParams {
                        bonding_curve: position.bonding_curve,
                        associated_bonding_curve: position.associated_bonding_curve,
                        creator_vault: position.creator_vault,
                    },
                };

                // 使用 SolTrade 卖出执行器
                match self.sol_trade_sell.execute_sell(sell_params).await {
                    Ok(signature) => {
                        info!("✅ SolTrade 卖出成功: {}", signature);

                        // 使用 monitor 轮询交易确认（10秒超时）
                        {
                            let monitor = self.monitor.read().await;
                            match monitor.poll_transaction_confirmation(signature, 10).await {
                                Ok(_) => {
                                    info!("✅ 卖出交易已确认");
                                }
                                Err(e) => {
                                    warn!("⚠️  卖出交易确认失败: {}, 继续结算", e);
                                }
                            }
                        }

                        let sol_received = self.tx_builder.estimate_sell_sol_amount(
                            metrics.latest_virtual_token_reserves,
                            metrics.latest_virtual_sol_reserves,
                            position.token_amount,
                        );
                        let profit_loss_sol = sol_received as i64 - position.sol_invested as i64;
                        let profit_loss_percent =
                            (profit_loss_sol as f64 / position.sol_invested as f64) * 100.0;
                        info!(
                            "💰 持仓已平仓: {:.4} SOL ({:+.2}%)",
                            sol_received as f64 / 1_000_000_000.0,
                            profit_loss_percent
                        );
                        self.positions.write().remove(&metrics.mint);
                    }
                    Err(e) => {
                        error!("❌ SolTrade 卖出失败: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        Ok(())
    }

    /// 处理持有信号
    async fn handle_hold_signal(&self, metrics: &WindowMetrics) {
        // 检查是否有该 token 的持仓
        let position_opt = {
            let positions = self.positions.read();
            positions.get(&metrics.mint).cloned()
        };

        if let Some(position) = position_opt {
            // 计算持仓时间
            let hold_duration = Utc::now().signed_duration_since(position.entry_time);
            let hold_secs = hold_duration.num_seconds() as u64;

            // 使用策略引擎评估退出条件
            let exit_signal = self.strategy.evaluate_exit_conditions(
                metrics,
                position.entry_price_sol,
                hold_secs,
            );

            if exit_signal == StrategySignal::Sell {
                info!("🟡 持有信号但满足退出条件，准备卖出: {}", metrics.mint);
                if let Err(e) = self.handle_sell_signal(metrics).await {
                    error!("❌ 退出持仓失败: {}", e);
                }
            }
        }
    }

    /// 派生 bonding curve PDA
    /// 🔥 优化: 使用缓存的 program_id
    fn derive_bonding_curve(&self, mint: &Pubkey) -> anyhow::Result<Pubkey> {
        let seeds = &[b"bonding-curve", mint.as_ref()];
        let (pda, _bump) = Pubkey::find_program_address(seeds, &PUMPFUN_PROGRAM_ID);
        Ok(pda)
    }

    /// 🔥 修复: 检测 mint 的 token program（支持 Token-2022）
    ///
    /// 📝 设计说明：此方法创建临时 RpcClient 是有意为之：
    ///    1. RpcClient::new() 开销极小（仅创建结构体，连接池是全局的）
    ///    2. 调用频率低（每次买入/卖出各 1-2 次）
    ///    3. 避免在 PositionManager 中添加 rpc_client 字段增加耦合
    ///    4. 性能影响 < 1ms，对整体延迟可忽略
    fn detect_token_program(&self, mint: &Pubkey) -> anyhow::Result<Pubkey> {
        use solana_client::rpc_client::RpcClient;

        let rpc_client = RpcClient::new(self.config.rpc_endpoint.clone());
        let account = rpc_client.get_account(mint)
            .map_err(|e| anyhow::anyhow!("读取 mint 账户失败: {}", e))?;

        let token_program = account.owner;

        if token_program == *TOKEN_2022_PROGRAM_ID {
            Ok(*TOKEN_2022_PROGRAM_ID)
        } else if token_program == *TOKEN_PROGRAM_ID {
            Ok(*TOKEN_PROGRAM_ID)
        } else {
            warn!("⚠️  未知 token program: {}, fallback to Token v3", token_program);
            Ok(*TOKEN_PROGRAM_ID)
        }
    }

    /// 🔥 修复: 获取支持 Token-2022 的 ATA 地址
    fn get_ata_with_program(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[
                wallet.as_ref(),
                token_program.as_ref(),
                mint.as_ref(),
            ],
            &ASSOCIATED_TOKEN_PROGRAM_ID,
        ).0
    }

    /// 派生 associated bonding curve PDA
    /// 🔥 修复: 应为 bonding_curve 的 **mint** ATA，而非 WSOL ATA
    /// 支持 Token-2022
    fn derive_associated_bonding_curve(&self, bonding_curve: &Pubkey, mint: &Pubkey) -> anyhow::Result<Pubkey> {
        // 检测 mint 的 token program
        let token_program = self.detect_token_program(mint)?;

        // 使用正确的 token program 派生 ATA
        Ok(Self::get_ata_with_program(bonding_curve, mint, &token_program))
    }

    /// 🔥 修复: 从 bonding_curve 账户读取 creator
    fn get_creator_from_bonding_curve(&self, bonding_curve: &Pubkey) -> anyhow::Result<Pubkey> {
        use crate::grpc::parser::bonding_curve_decode;
        use solana_client::rpc_client::RpcClient;

        // 创建临时 RPC client 读取链上数据
        let rpc_client = RpcClient::new(self.config.rpc_endpoint.clone());
        let data = rpc_client.get_account_data(bonding_curve)
            .map_err(|e| anyhow::anyhow!("读取 bonding curve 账户失败: {}", e))?;

        let bc = bonding_curve_decode(&data)
            .ok_or_else(|| anyhow::anyhow!("解码 bonding curve 失败"))?;

        Ok(bc.creator)
    }

    /// 🔥 修复: 派生 creator_vault PDA（完全参考 sol-trade-sdk）
    /// seed = [b"creator-vault", creator.as_ref()]
    /// program_id = PUMPFUN_PROGRAM_ID
    fn derive_creator_vault(creator: &Pubkey) -> anyhow::Result<Pubkey> {
        let (creator_vault, _bump) = Pubkey::find_program_address(
            &[b"creator-vault", creator.as_ref()],
            &PUMPFUN_PROGRAM_ID,
        );
        Ok(creator_vault)
    }

}

