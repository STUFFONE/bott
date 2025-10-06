use log::{debug, info, warn};
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::aggregator::Aggregator;
use crate::config::Config;
use crate::dynamic_strategy::{DynamicStrategyConfig, DynamicStrategyEngine};
use crate::types::{BondingCurveState, StrategySignal, WindowMetrics};

/// 策略引擎（增强版）
///
/// 集成了动态策略引擎和高级指标
pub struct StrategyEngine {
    config: Arc<Config>,
    signal_tx: mpsc::Sender<(Arc<WindowMetrics>, StrategySignal)>,
    /// 动态策略引擎
    dynamic_strategy: Arc<RwLock<DynamicStrategyEngine>>,
    /// 聚合器引用（用于获取高级指标，保留作为备用）
    #[allow(dead_code)]
    aggregator: Arc<Aggregator>,
}

impl StrategyEngine {
    pub fn new(
        config: Arc<Config>,
        signal_tx: mpsc::Sender<(Arc<WindowMetrics>, StrategySignal)>,
        aggregator: Arc<Aggregator>,
    ) -> Self {
        // 从配置创建动态策略引擎
        let dynamic_config = Self::create_dynamic_config_from_env(&config);
        let dynamic_strategy = Arc::new(RwLock::new(DynamicStrategyEngine::new(dynamic_config)));

        info!("🎯 策略引擎已初始化（增强版）");
        info!("   ✅ 动态策略引擎已启用");
        info!("   策略模式: {}", config.dynamic_strategy_mode);

        Self {
            config,
            signal_tx,
            dynamic_strategy,
            aggregator,
        }
    }

    /// 从环境变量创建动态策略配置
    fn create_dynamic_config_from_env(config: &Config) -> DynamicStrategyConfig {
        use crate::dynamic_strategy::{BuyTriggers, SellTriggers, AdaptiveParams, StrategyMode};

        // 🔥 优先使用布尔值开关（如果启用）
        let mode = if config.enable_custom_mode {
            info!("🎯 启用自定义模式 (ENABLE_CUSTOM_MODE=true)");
            StrategyMode::Custom
        } else if config.enable_conservative_mode {
            info!("🎯 启用保守模式 (ENABLE_CONSERVATIVE_MODE=true)");
            StrategyMode::Conservative
        } else if config.enable_aggressive_mode {
            info!("🎯 启用激进模式 (ENABLE_AGGRESSIVE_MODE=true)");
            StrategyMode::Aggressive
        } else if config.enable_balanced_mode {
            info!("🎯 启用平衡模式 (ENABLE_BALANCED_MODE=true)");
            StrategyMode::Balanced
        } else {
            // 如果所有布尔值都是false，回退到字符串模式
            info!("⚠️  所有模式开关都是false，使用 DYNAMIC_STRATEGY_MODE={}", config.dynamic_strategy_mode);
            match config.dynamic_strategy_mode.as_str() {
                "conservative" => StrategyMode::Conservative,
                "aggressive" => StrategyMode::Aggressive,
                "custom" => StrategyMode::Custom,
                _ => StrategyMode::Balanced,
            }
        };

        let (buy_triggers, sell_triggers) = match mode {
            StrategyMode::Conservative => (
                BuyTriggers {
                    min_buy_ratio: config.conservative_min_buy_ratio,
                    min_net_inflow_sol: config.net_inflow_threshold_sol,
                    min_acceleration: config.conservative_min_acceleration,
                    max_slippage: config.conservative_max_slippage,
                    min_high_frequency_trades: config.conservative_min_high_frequency_trades,
                    min_liquidity_depth: config.conservative_min_liquidity_depth,
                    max_price_impact: config.conservative_max_price_impact,
                    min_composite_score: config.conservative_min_composite_score,
                },
                SellTriggers {
                    take_profit_multiplier: config.take_profit_multiplier,
                    stop_loss_multiplier: config.stop_loss_multiplier,
                    min_hold_duration_secs: config.hold_min_duration_secs,
                    max_hold_duration_secs: config.hold_max_duration_secs,
                    momentum_decay_threshold: config.exit_buy_ratio_threshold,
                },
            ),
            StrategyMode::Balanced => (
                BuyTriggers {
                    min_buy_ratio: config.balanced_min_buy_ratio,
                    min_net_inflow_sol: config.net_inflow_threshold_sol,
                    min_acceleration: config.balanced_min_acceleration,
                    max_slippage: config.balanced_max_slippage,
                    min_high_frequency_trades: config.balanced_min_high_frequency_trades,
                    min_liquidity_depth: config.balanced_min_liquidity_depth,
                    max_price_impact: config.balanced_max_price_impact,
                    min_composite_score: config.balanced_min_composite_score,
                },
                SellTriggers {
                    take_profit_multiplier: config.take_profit_multiplier,
                    stop_loss_multiplier: config.stop_loss_multiplier,
                    min_hold_duration_secs: config.hold_min_duration_secs,
                    max_hold_duration_secs: config.hold_max_duration_secs,
                    momentum_decay_threshold: config.exit_buy_ratio_threshold,
                },
            ),
            StrategyMode::Aggressive => (
                BuyTriggers {
                    min_buy_ratio: config.aggressive_min_buy_ratio,
                    min_net_inflow_sol: config.net_inflow_threshold_sol,
                    min_acceleration: config.aggressive_min_acceleration,
                    max_slippage: config.aggressive_max_slippage,
                    min_high_frequency_trades: config.aggressive_min_high_frequency_trades,
                    min_liquidity_depth: config.aggressive_min_liquidity_depth,
                    max_price_impact: config.aggressive_max_price_impact,
                    min_composite_score: config.aggressive_min_composite_score,
                },
                SellTriggers {
                    take_profit_multiplier: config.take_profit_multiplier,
                    stop_loss_multiplier: config.stop_loss_multiplier,
                    min_hold_duration_secs: config.hold_min_duration_secs,
                    max_hold_duration_secs: config.hold_max_duration_secs,
                    momentum_decay_threshold: config.exit_buy_ratio_threshold,
                },
            ),
            StrategyMode::Custom => (
                BuyTriggers {
                    min_buy_ratio: config.custom_min_buy_ratio,
                    min_net_inflow_sol: config.net_inflow_threshold_sol,
                    min_acceleration: config.custom_min_acceleration,
                    max_slippage: config.custom_max_slippage,
                    min_high_frequency_trades: config.custom_min_high_frequency_trades,
                    min_liquidity_depth: config.custom_min_liquidity_depth,
                    max_price_impact: config.custom_max_price_impact,
                    min_composite_score: config.custom_min_composite_score,
                },
                SellTriggers {
                    take_profit_multiplier: config.take_profit_multiplier,
                    stop_loss_multiplier: config.stop_loss_multiplier,
                    min_hold_duration_secs: config.hold_min_duration_secs,
                    max_hold_duration_secs: config.hold_max_duration_secs,
                    momentum_decay_threshold: config.exit_buy_ratio_threshold,
                },
            ),
        };

        DynamicStrategyConfig {
            mode,
            buy_triggers,
            sell_triggers,
            adaptive_params: AdaptiveParams {
                enable_volatility_adaptation: true,
                enable_time_adaptation: true,
                enable_success_feedback: true,
                volatility_adjustment_factor: 1.0,
            },
        }
    }

    /// 启动策略引擎
    pub async fn start(&self, mut metrics_rx: mpsc::Receiver<Arc<WindowMetrics>>) {
        info!("Strategy engine started");

        while let Some(metrics_arc) = metrics_rx.recv().await {
            let signal = self.evaluate_metrics(&metrics_arc);

            if signal != StrategySignal::None {
                debug!(
                    "Signal generated for {}: {:?}",
                    metrics_arc.mint, signal
                );

                if let Err(e) = self.signal_tx.send((metrics_arc, signal)).await {
                    log::error!("Failed to send signal: {}", e);
                }
            }
        }
    }

    /// 评估指标并生成信号（增强版）
    fn evaluate_metrics(&self, metrics: &WindowMetrics) -> StrategySignal {
        // 🎯 阈值触发策略：优先级最高
        if self.config.enable_threshold_trigger {
            if let Some(buy_amount) = metrics.threshold_buy_amount {
                info!("🎯 阈值触发策略命中！");
                info!("   Mint: {}", metrics.mint);
                info!("   买入金额: {:.4} SOL", buy_amount);
                info!("   立即执行买入！");
                return StrategySignal::Buy;
            }
        }

        // 🚀 首波狙击逻辑：检测新币的第一波大额流入
        if self.config.enable_first_wave_sniper {
            let is_first_wave = metrics.event_count <= 5; // 前5笔交易视为首波
            if is_first_wave {
                let net_inflow_sol = metrics.net_inflow_sol as f64 / 1_000_000_000.0;

                // 首波快速狙击条件（可配置）：
                // 1. 有资金流入（大于阈值 × 倍数）
                // 2. 买占比 >= 配置的阈值
                let first_wave_inflow_threshold = self.config.net_inflow_threshold_sol * self.config.first_wave_inflow_multiplier;

                if net_inflow_sol >= first_wave_inflow_threshold && metrics.buy_ratio >= self.config.first_wave_buy_ratio {
                    info!("🚀 首波狙击触发！");
                    info!("   事件数: {}", metrics.event_count);
                    info!("   净流入: {:.4} SOL (阈值: {:.4} SOL)",
                        net_inflow_sol, first_wave_inflow_threshold);
                    info!("   买占比: {:.2}% (阈值: {:.2}%)",
                        metrics.buy_ratio * 100.0, self.config.first_wave_buy_ratio * 100.0);
                    info!("   🎯 立即买入！");
                    return StrategySignal::Buy;
                } else {
                    debug!("首波监控中... 事件数: {}, 净流入: {:.4} SOL, 买占比: {:.2}%",
                        metrics.event_count, net_inflow_sol, metrics.buy_ratio * 100.0);
                }
            }
        }

        // 检查是否有足够的事件数据（常规策略）
        if metrics.event_count < 3 {
            return StrategySignal::None;
        }

        // 尝试获取高级指标（优先使用已传递的指标）
        let advanced_metrics = if let Some(ref adv) = metrics.advanced_metrics {
            Some(adv)
        } else {
            // Fallback: 如果 metrics 中没有，尝试从 aggregator 获取
            debug!("⚠️  metrics 中无高级指标，从 aggregator 获取");
            None  // aggregator.get_advanced_metrics() 返回的是临时值，无法引用
        };

        // 如果有高级指标，使用动态策略引擎
        if let Some(advanced) = advanced_metrics {
            let mut dynamic = self.dynamic_strategy.write();
            let (should_buy, confidence) = dynamic.evaluate_buy(metrics, advanced);

            if should_buy {
                info!("✅ 动态策略引擎: 买入信号 (置信度: {:.2}%)", confidence * 100.0);
                return StrategySignal::Buy;
            } else {
                debug!("❌ 动态策略引擎: 不满足买入条件");
                return StrategySignal::None;
            }
        }

        // 如果没有高级指标，使用传统策略（向后兼容）
        debug!("⚠️  高级指标不足，使用传统策略");

        // 条件 1: 买入占比检查
        if metrics.buy_ratio < self.config.buy_ratio_threshold {
            return StrategySignal::None;
        }

        // 条件 2: 净流入检查
        let net_inflow_sol = metrics.net_inflow_sol as f64 / 1_000_000_000.0;
        if net_inflow_sol < self.config.net_inflow_threshold_sol {
            return StrategySignal::None;
        }

        // 条件 3: 加速度检查（如果启用）
        if self.config.acceleration_required {
            if metrics.acceleration < self.config.acceleration_multiplier {
                return StrategySignal::None;
            }
        }

        // 条件 4: 滑点检查
        let curve_state = BondingCurveState {
            virtual_sol_reserves: metrics.latest_virtual_sol_reserves,
            virtual_token_reserves: metrics.latest_virtual_token_reserves,
        };

        let snipe_amount = self.config.get_snipe_amount_lamports();
        let estimated_slippage = curve_state.estimate_buy_slippage(snipe_amount);

        if estimated_slippage > self.config.max_slippage_percent {
            debug!(
                "Slippage too high for {}: {:.2}% > {:.2}%",
                metrics.mint, estimated_slippage, self.config.max_slippage_percent
            );
            return StrategySignal::None;
        }

        // 所有条件满足，生成买入信号
        info!(
            "🎯 BUY SIGNAL for {} - Buy Ratio: {:.2}%, Net Inflow: {:.4} SOL, Acceleration: {:.2}x, Slippage: {:.2}%",
            metrics.mint,
            metrics.buy_ratio * 100.0,
            net_inflow_sol,
            metrics.acceleration,
            estimated_slippage
        );

        StrategySignal::Buy
    }

    /// 评估退出条件
    pub fn evaluate_exit_conditions(
        &self,
        metrics: &WindowMetrics,
        entry_price_sol: f64,
        hold_duration_secs: u64,
    ) -> StrategySignal {
        // 使用动态策略的卖出触发条件
        let dynamic_strategy = self.dynamic_strategy.read();
        let triggers = dynamic_strategy.get_sell_triggers();

        // 1. 检查最小持仓时间
        if hold_duration_secs < triggers.min_hold_duration_secs {
            return StrategySignal::Hold;
        }

        // 2. 检查最大持仓时间
        if hold_duration_secs >= triggers.max_hold_duration_secs {
            info!("⏰ TIMEOUT EXIT for {} - Held for {}s", metrics.mint, hold_duration_secs);
            return StrategySignal::Sell;
        }

        // 3. 计算当前价格
        if metrics.latest_virtual_sol_reserves > 0 && metrics.latest_virtual_token_reserves > 0 {
            let current_price_sol = metrics.latest_virtual_sol_reserves as f64
                / metrics.latest_virtual_token_reserves as f64;

            // 🔥 优化: 构建曲线状态用于滑点检查
            let curve_state = BondingCurveState {
                virtual_sol_reserves: metrics.latest_virtual_sol_reserves,
                virtual_token_reserves: metrics.latest_virtual_token_reserves,
            };

            // 4. 止盈检查（加流动性检查）
            if triggers.take_profit_multiplier > 0.0 {
                let take_profit_price = entry_price_sol * triggers.take_profit_multiplier;
                if current_price_sol >= take_profit_price {
                    // 🔥 优化: 检查滑点是否可接受
                    let estimated_slippage = curve_state.estimate_buy_slippage(
                        self.config.get_snipe_amount_lamports() // 使用买入金额估算卖出滑点
                    );

                    if estimated_slippage > self.config.max_slippage_percent {
                        warn!("💰 达到止盈价格但滑点过高 for {} - 价格: {:.8} SOL ({}x), 滑点: {:.2}%",
                            metrics.mint, current_price_sol, triggers.take_profit_multiplier, estimated_slippage);
                        warn!("   继续持有等待流动性改善");
                        return StrategySignal::Hold;
                    }

                    info!("💰 TAKE PROFIT for {} - Price: {:.8} SOL ({}x), Slippage: {:.2}%",
                        metrics.mint, current_price_sol, triggers.take_profit_multiplier, estimated_slippage);
                    return StrategySignal::Sell;
                }
            }

            // 5. 止损检查（加流动性检查）
            if triggers.stop_loss_multiplier > 0.0 {
                let stop_loss_price = entry_price_sol * triggers.stop_loss_multiplier;
                if current_price_sol <= stop_loss_price {
                    // 🔥 优化: 止损时也检查滑点，避免恐慌性抛售造成更大损失
                    let estimated_slippage = curve_state.estimate_buy_slippage(
                        self.config.get_snipe_amount_lamports()
                    );

                    if estimated_slippage > self.config.max_slippage_percent * 2.0 {
                        // 止损时滑点容忍度 2x
                        warn!("🛑 达到止损价格但滑点极高 for {} - 价格: {:.8} SOL ({}x), 滑点: {:.2}%",
                            metrics.mint, current_price_sol, triggers.stop_loss_multiplier, estimated_slippage);
                        warn!("   等待流动性改善后再卖出（避免更大损失）");
                        return StrategySignal::Hold;
                    }

                    warn!("🛑 STOP LOSS for {} - Price: {:.8} SOL ({}x), Slippage: {:.2}%",
                        metrics.mint, current_price_sol, triggers.stop_loss_multiplier, estimated_slippage);
                    return StrategySignal::Sell;
                }
            }
        }

        // 6. 动能衰减检查
        if metrics.buy_ratio < triggers.momentum_decay_threshold {
            info!("📉 MOMENTUM DECAY for {} - Buy ratio dropped to {:.2}%",
                metrics.mint, metrics.buy_ratio * 100.0);
            return StrategySignal::Sell;
        }

        StrategySignal::Hold
    }
}

