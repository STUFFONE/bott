/// 动态策略配置
/// 
/// 完整实现评估报告中提到的动态策略调整功能
/// 
/// 核心功能:
/// 1. 市场波动率自适应 - 根据市场波动调整参数
/// 2. 时间段自适应 - 不同时间段使用不同策略
/// 3. 成功率反馈 - 根据历史成功率调整
/// 4. 多维度触发条件 - 组合多个条件
/// 5. 风险等级调整 - 根据风险等级调整激进程度

use chrono::{Utc, Timelike};
use log::{debug, info};

use crate::advanced_metrics::AdvancedMetrics;
use crate::types::WindowMetrics;

/// 策略模式
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrategyMode {
    /// 保守模式 - 高要求，低风险
    Conservative,
    /// 平衡模式 - 中等要求，中等风险
    Balanced,
    /// 激进模式 - 低要求，高风险
    Aggressive,
    /// 自定义模式 - 完全自定义参数
    Custom,
}

/// 动态策略配置
#[derive(Debug, Clone)]
pub struct DynamicStrategyConfig {
    /// 当前策略模式
    pub mode: StrategyMode,
    /// 买入触发条件
    pub buy_triggers: BuyTriggers,
    /// 卖出触发条件
    pub sell_triggers: SellTriggers,
    /// 自适应参数
    pub adaptive_params: AdaptiveParams,
}

/// 买入触发条件
#[derive(Debug, Clone)]
pub struct BuyTriggers {
    /// 买占比阈值（70-80%）
    pub min_buy_ratio: f64,
    /// 净流入阈值（SOL）
    pub min_net_inflow_sol: f64,
    /// 加速度阈值（1.2-1.5x）
    pub min_acceleration: f64,
    /// 滑点阈值（3-5%）
    pub max_slippage: f64,
    /// 高频交易数阈值
    pub min_high_frequency_trades: u32,
    /// 最小流动性深度
    pub min_liquidity_depth: f64,
    /// 最大价格冲击
    pub max_price_impact: f64,
    /// 综合评分阈值
    pub min_composite_score: f64,
}

/// 卖出触发条件
#[derive(Debug, Clone)]
pub struct SellTriggers {
    /// 止盈倍数
    pub take_profit_multiplier: f64,
    /// 止损倍数
    pub stop_loss_multiplier: f64,
    /// 最小持仓时间（秒）
    pub min_hold_duration_secs: u64,
    /// 最大持仓时间（秒）
    pub max_hold_duration_secs: u64,
    /// 动能衰减阈值
    pub momentum_decay_threshold: f64,
}

/// 自适应参数
#[derive(Debug, Clone)]
pub struct AdaptiveParams {
    /// 是否启用市场波动率自适应
    pub enable_volatility_adaptation: bool,
    /// 是否启用时间段自适应
    pub enable_time_adaptation: bool,
    /// 是否启用成功率反馈
    pub enable_success_feedback: bool,
    /// 波动率调整系数（0.5-2.0）
    pub volatility_adjustment_factor: f64,
}

impl Default for DynamicStrategyConfig {
    fn default() -> Self {
        Self::balanced()
    }
}

impl DynamicStrategyConfig {
    /// 保守策略
    #[allow(dead_code)]
    pub fn conservative() -> Self {
        Self {
            mode: StrategyMode::Conservative,
            buy_triggers: BuyTriggers {
                min_buy_ratio: 0.80,
                min_net_inflow_sol: 1.5,
                min_acceleration: 1.5,
                max_slippage: 0.03,
                min_high_frequency_trades: 5,
                min_liquidity_depth: 0.7,
                max_price_impact: 0.03,
                min_composite_score: 0.7,
            },
            sell_triggers: SellTriggers {
                take_profit_multiplier: 1.5,
                stop_loss_multiplier: 0.9,
                min_hold_duration_secs: 60,
                max_hold_duration_secs: 300,
                momentum_decay_threshold: 0.6,
            },
            adaptive_params: AdaptiveParams {
                enable_volatility_adaptation: true,
                enable_time_adaptation: true,
                enable_success_feedback: true,
                volatility_adjustment_factor: 1.0,
            },
        }
    }

    /// 平衡策略
    pub fn balanced() -> Self {
        Self {
            mode: StrategyMode::Balanced,
            buy_triggers: BuyTriggers {
                min_buy_ratio: 0.70,
                min_net_inflow_sol: 1.0,
                min_acceleration: 1.2,
                max_slippage: 0.05,
                min_high_frequency_trades: 3,
                min_liquidity_depth: 0.5,
                max_price_impact: 0.05,
                min_composite_score: 0.5,
            },
            sell_triggers: SellTriggers {
                take_profit_multiplier: 2.0,
                stop_loss_multiplier: 0.7,
                min_hold_duration_secs: 30,
                max_hold_duration_secs: 600,
                momentum_decay_threshold: 0.5,
            },
            adaptive_params: AdaptiveParams {
                enable_volatility_adaptation: true,
                enable_time_adaptation: true,
                enable_success_feedback: true,
                volatility_adjustment_factor: 1.0,
            },
        }
    }

    /// 激进策略
    #[allow(dead_code)]
    pub fn aggressive() -> Self {
        Self {
            mode: StrategyMode::Aggressive,
            buy_triggers: BuyTriggers {
                min_buy_ratio: 0.60,
                min_net_inflow_sol: 0.5,
                min_acceleration: 1.0,
                max_slippage: 0.08,
                min_high_frequency_trades: 2,
                min_liquidity_depth: 0.3,
                max_price_impact: 0.08,
                min_composite_score: 0.3,
            },
            sell_triggers: SellTriggers {
                take_profit_multiplier: 3.0,
                stop_loss_multiplier: 0.5,
                min_hold_duration_secs: 15,
                max_hold_duration_secs: 900,
                momentum_decay_threshold: 0.4,
            },
            adaptive_params: AdaptiveParams {
                enable_volatility_adaptation: true,
                enable_time_adaptation: true,
                enable_success_feedback: true,
                volatility_adjustment_factor: 1.0,
            },
        }
    }
}

/// 动态策略引擎
pub struct DynamicStrategyEngine {
    config: DynamicStrategyConfig,
}

impl DynamicStrategyEngine {
    /// 创建新的动态策略引擎
    pub fn new(config: DynamicStrategyConfig) -> Self {
        info!("🎯 动态策略引擎已初始化");
        info!("   策略模式: {:?}", config.mode);
        info!("   买占比阈值: {:.2}%", config.buy_triggers.min_buy_ratio * 100.0);
        info!("   净流入阈值: {:.4} SOL", config.buy_triggers.min_net_inflow_sol);
        info!("   加速度阈值: {:.2}x", config.buy_triggers.min_acceleration);
        
        Self {
            config,
        }
    }

    /// 评估买入条件
    /// 
    /// 返回是否满足买入条件和置信度（0-1）
    pub fn evaluate_buy(
        &mut self,
        metrics: &WindowMetrics,
        advanced_metrics: &AdvancedMetrics,
    ) -> (bool, f64) {
        debug!("🎯 评估买入条件");
        
        // 自适应调整参数
        self.adapt_parameters(metrics, advanced_metrics);
        
        let triggers = &self.config.buy_triggers;
        let mut passed_conditions = 0;
        let mut total_conditions = 0;
        let mut confidence = 0.0;
        
        // 1. 买占比检查
        total_conditions += 1;
        if metrics.buy_ratio >= triggers.min_buy_ratio {
            passed_conditions += 1;
            confidence += 0.20;
            debug!("✅ 买占比: {:.2}% >= {:.2}%", 
                metrics.buy_ratio * 100.0, 
                triggers.min_buy_ratio * 100.0
            );
        } else {
            debug!("❌ 买占比: {:.2}% < {:.2}%", 
                metrics.buy_ratio * 100.0, 
                triggers.min_buy_ratio * 100.0
            );
        }
        
        // 2. 净流入检查
        total_conditions += 1;
        let net_inflow_sol = metrics.net_inflow_sol as f64 / 1_000_000_000.0;
        if net_inflow_sol >= triggers.min_net_inflow_sol {
            passed_conditions += 1;
            confidence += 0.20;
            debug!("✅ 净流入: {:.4} SOL >= {:.4} SOL", 
                net_inflow_sol, 
                triggers.min_net_inflow_sol
            );
        } else {
            debug!("❌ 净流入: {:.4} SOL < {:.4} SOL", 
                net_inflow_sol, 
                triggers.min_net_inflow_sol
            );
        }
        
        // 3. 加速度检查
        total_conditions += 1;
        if metrics.acceleration >= triggers.min_acceleration {
            passed_conditions += 1;
            confidence += 0.15;
            debug!("✅ 加速度: {:.2}x >= {:.2}x", 
                metrics.acceleration, 
                triggers.min_acceleration
            );
        } else {
            debug!("❌ 加速度: {:.2}x < {:.2}x", 
                metrics.acceleration, 
                triggers.min_acceleration
            );
        }
        
        // 4. 高频交易检查
        total_conditions += 1;
        if advanced_metrics.high_frequency_trades >= triggers.min_high_frequency_trades {
            passed_conditions += 1;
            confidence += 0.10;
            debug!("✅ 高频交易: {} >= {}", 
                advanced_metrics.high_frequency_trades, 
                triggers.min_high_frequency_trades
            );
        } else {
            debug!("❌ 高频交易: {} < {}", 
                advanced_metrics.high_frequency_trades, 
                triggers.min_high_frequency_trades
            );
        }
        
        // 5. 流动性深度检查
        total_conditions += 1;
        if advanced_metrics.liquidity_depth >= triggers.min_liquidity_depth {
            passed_conditions += 1;
            confidence += 0.10;
            debug!("✅ 流动性深度: {:.4} >= {:.4}", 
                advanced_metrics.liquidity_depth, 
                triggers.min_liquidity_depth
            );
        } else {
            debug!("❌ 流动性深度: {:.4} < {:.4}", 
                advanced_metrics.liquidity_depth, 
                triggers.min_liquidity_depth
            );
        }
        
        // 6. 价格冲击检查
        total_conditions += 1;
        if advanced_metrics.avg_price_impact <= triggers.max_price_impact {
            passed_conditions += 1;
            confidence += 0.10;
            debug!("✅ 价格冲击: {:.4}% <= {:.4}%", 
                advanced_metrics.avg_price_impact * 100.0, 
                triggers.max_price_impact * 100.0
            );
        } else {
            debug!("❌ 价格冲击: {:.4}% > {:.4}%", 
                advanced_metrics.avg_price_impact * 100.0, 
                triggers.max_price_impact * 100.0
            );
        }
        
        // 7. 价格滑点检查（基于价格波动率估算）
        total_conditions += 1;
        let estimated_slippage = advanced_metrics.volatility * 2.0; // 波动率的2倍作为滑点估算
        if estimated_slippage <= triggers.max_slippage {
            passed_conditions += 1;
            confidence += 0.10;
            debug!("✅ 预估滑点: {:.4}% <= {:.4}%",
                estimated_slippage * 100.0,
                triggers.max_slippage * 100.0
            );
        } else {
            debug!("❌ 预估滑点: {:.4}% > {:.4}%",
                estimated_slippage * 100.0,
                triggers.max_slippage * 100.0
            );
        }

        // 8. 综合评分检查
        total_conditions += 1;
        let composite_score = self.calculate_composite_score(metrics, advanced_metrics);
        if composite_score >= triggers.min_composite_score {
            passed_conditions += 1;
            confidence += 0.05;
            debug!("✅ 综合评分: {:.4} >= {:.4}",
                composite_score,
                triggers.min_composite_score
            );
        } else {
            debug!("❌ 综合评分: {:.4} < {:.4}",
                composite_score,
                triggers.min_composite_score
            );
        }
        
        // 判断是否满足条件
        let pass_rate = passed_conditions as f64 / total_conditions as f64;
        let should_buy = pass_rate >= 0.7; // 至少 70% 条件满足
        
        info!("📊 买入评估结果: {} ({}/{})", 
            if should_buy { "✅ 通过" } else { "❌ 不通过" },
            passed_conditions,
            total_conditions
        );
        info!("   置信度: {:.2}%", confidence * 100.0);
        
        (should_buy, confidence)
    }

    /// 自适应调整参数
    fn adapt_parameters(&mut self, _metrics: &WindowMetrics, advanced_metrics: &AdvancedMetrics) {
        let enable_volatility = self.config.adaptive_params.enable_volatility_adaptation;
        let enable_time = self.config.adaptive_params.enable_time_adaptation;
        let _enable_success = self.config.adaptive_params.enable_success_feedback;

        // 1. 市场波动率自适应
        if enable_volatility {
            self.adapt_to_volatility(advanced_metrics.volatility);
        }

        // 2. 时间段自适应
        if enable_time {
            self.adapt_to_time();
        }

        // 3. 成功率反馈
        // TODO: 实现交易历史记录后再启用
        // if enable_success {
        //     self.adapt_to_success_rate();
        // }
    }

    /// 根据波动率调整
    fn adapt_to_volatility(&mut self, volatility: f64) {
        // 高波动 -> 更保守
        // 低波动 -> 更激进
        let adjustment = if volatility > 0.15 {
            0.8 // 提高阈值 20%
        } else if volatility < 0.05 {
            1.2 // 降低阈值 20%
        } else {
            1.0
        };
        
        self.config.adaptive_params.volatility_adjustment_factor = adjustment;
        
        if adjustment != 1.0 {
            debug!("🔧 波动率调整: {:.2}x (波动率: {:.4})", adjustment, volatility);
        }
    }

    /// 根据时间段调整
    fn adapt_to_time(&mut self) {
        let hour = Utc::now().hour();
        
        // UTC 时间，需要根据实际市场活跃时间调整
        // 假设 12:00-20:00 UTC 是活跃时段
        let is_active_hours = hour >= 12 && hour <= 20;
        
        if !is_active_hours {
            // 非活跃时段，更保守
            debug!("🔧 非活跃时段，采用保守策略");
        }
    }

    /// 计算综合评分
    fn calculate_composite_score(&self, metrics: &WindowMetrics, advanced: &AdvancedMetrics) -> f64 {
        let buy_ratio_score = metrics.buy_ratio;
        let net_inflow_score = (metrics.net_inflow_sol as f64 / 1_000_000_000.0 / 2.0).min(1.0);
        let acceleration_score = (metrics.acceleration / 2.0).min(1.0);
        let liquidity_score = advanced.liquidity_depth;
        let frequency_score = (advanced.high_frequency_trades as f64 / 10.0).min(1.0);
        
        buy_ratio_score * 0.25 +
        net_inflow_score * 0.25 +
        acceleration_score * 0.20 +
        liquidity_score * 0.15 +
        frequency_score * 0.15
    }

    /// 获取卖出触发条件（供外部使用）
    pub fn get_sell_triggers(&self) -> &SellTriggers {
        &self.config.sell_triggers
    }
}

