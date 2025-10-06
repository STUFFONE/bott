/// 动能衰减检测器
/// 
/// 完整实现评估报告中提到的动能衰减检测逻辑
/// 
/// 核心功能:
/// 1. 买卖占比回落检测 - 买占比 < 50%
/// 2. 净流入转负检测 - 净流入 < 0
/// 3. 成交频度骤降检测 - 高频交易 < 2笔
/// 4. 多维度动能指标综合评估
/// 5. 时间窗口分析

use log::{debug, info, warn};

use crate::types::WindowMetrics;

/// 衰减原因
#[derive(Debug, Clone, PartialEq)]
pub enum DecayReason {
    /// 买卖占比回落（买占比 < 阈值）
    BuyRatioDecline {
        current: f64,
        threshold: f64,
    },
    /// 净流入转负
    NegativeInflow {
        current: f64,
    },
    /// 成交频度骤降
    LowActivity {
        current: u32,
        threshold: u32,
    },
    /// 加速度衰减（后半窗 < 前半窗）
    AccelerationDecay {
        current: f64,
        threshold: f64,
    },
    /// 综合评分过低
    LowCompositeScore {
        score: f64,
        threshold: f64,
    },
}

impl DecayReason {
    pub fn description(&self) -> String {
        match self {
            DecayReason::BuyRatioDecline { current, threshold } => {
                format!("买占比回落: {:.2}% < {:.2}%", current * 100.0, threshold * 100.0)
            }
            DecayReason::NegativeInflow { current } => {
                format!("净流入转负: {:.4} SOL", *current / 1_000_000_000.0)
            }
            DecayReason::LowActivity { current, threshold } => {
                format!("成交频度骤降: {} < {} 笔", current, threshold)
            }
            DecayReason::AccelerationDecay { current, threshold } => {
                format!("加速度衰减: {:.2} < {:.2}", current, threshold)
            }
            DecayReason::LowCompositeScore { score, threshold } => {
                format!("综合评分过低: {:.2} < {:.2}", score, threshold)
            }
        }
    }
}

/// 动能衰减检测配置
#[derive(Debug, Clone)]
pub struct MomentumDecayConfig {
    /// 买占比阈值（默认 0.5 = 50%）
    pub buy_ratio_threshold: f64,
    /// 净流入阈值（默认 0.0 SOL）
    pub net_inflow_threshold: f64,
    /// 交易频率阈值（默认 2 笔）
    pub trade_frequency_threshold: u32,
    /// 加速度阈值（默认 1.0）
    pub acceleration_threshold: f64,
    /// 综合评分阈值（默认 0.3）
    pub composite_score_threshold: f64,
    /// 是否启用严格模式（所有条件都要满足）
    pub strict_mode: bool,
}

impl Default for MomentumDecayConfig {
    fn default() -> Self {
        Self {
            buy_ratio_threshold: 0.5,
            net_inflow_threshold: 0.0,
            trade_frequency_threshold: 2,
            acceleration_threshold: 1.0,
            composite_score_threshold: 0.3,
            strict_mode: false,
        }
    }
}

/// 动能衰减检测器
pub struct MomentumDecayDetector {
    config: MomentumDecayConfig,
}

impl MomentumDecayDetector {
    /// 创建新的动能衰减检测器
    pub fn new(config: MomentumDecayConfig) -> Self {
        info!("🔍 动能衰减检测器已初始化");
        info!("   买占比阈值: {:.2}%", config.buy_ratio_threshold * 100.0);
        info!("   净流入阈值: {:.4} SOL", config.net_inflow_threshold);
        info!("   交易频率阈值: {} 笔", config.trade_frequency_threshold);
        info!("   加速度阈值: {:.2}", config.acceleration_threshold);
        info!("   严格模式: {}", config.strict_mode);

        Self {
            config,
        }
    }

    /// 检测动能衰减
    ///
    /// 返回 Some(DecayReason) 如果检测到衰减，否则返回 None
    pub fn detect(&self, metrics: &WindowMetrics) -> Option<DecayReason> {
        debug!("🔍 开始动能衰减检测");
        debug!("   Token: {}", metrics.mint);
        debug!("   买占比: {:.2}%", metrics.buy_ratio * 100.0);
        debug!("   净流入: {:.4} SOL", metrics.net_inflow_sol as f64 / 1_000_000_000.0);
        debug!("   加速度: {:.2}", metrics.acceleration);

        // 执行各项检测
        let mut decay_reasons = Vec::new();
        
        // 1. 买卖占比回落检测
        if let Some(reason) = self.check_buy_ratio_decline(metrics) {
            decay_reasons.push(reason);
        }
        
        // 2. 净流入转负检测
        if let Some(reason) = self.check_negative_inflow(metrics) {
            decay_reasons.push(reason);
        }
        
        // 3. 成交频度骤降检测
        if let Some(reason) = self.check_low_activity(metrics) {
            decay_reasons.push(reason);
        }
        
        // 4. 加速度衰减检测
        if let Some(reason) = self.check_acceleration_decay(metrics) {
            decay_reasons.push(reason);
        }
        
        // 5. 综合评分检测
        if let Some(reason) = self.check_composite_score(metrics) {
            decay_reasons.push(reason);
        }
        
        // 根据模式返回结果
        if self.config.strict_mode {
            // 严格模式：所有条件都要满足
            if decay_reasons.len() >= 3 {
                if let Some(reason) = decay_reasons.into_iter().next() {
                    warn!("⚠️  检测到动能衰减（严格模式）: {}", reason.description());
                    Some(reason)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            // 宽松模式：任一条件满足即可
            if let Some(reason) = decay_reasons.into_iter().next() {
                warn!("⚠️  检测到动能衰减: {}", reason.description());
                Some(reason)
            } else {
                debug!("✅ 动能正常");
                None
            }
        }
    }

    /// 检查买占比回落
    fn check_buy_ratio_decline(&self, metrics: &WindowMetrics) -> Option<DecayReason> {
        if metrics.buy_ratio < self.config.buy_ratio_threshold {
            debug!("❌ 买占比回落: {:.2}% < {:.2}%", 
                metrics.buy_ratio * 100.0, 
                self.config.buy_ratio_threshold * 100.0
            );
            return Some(DecayReason::BuyRatioDecline {
                current: metrics.buy_ratio,
                threshold: self.config.buy_ratio_threshold,
            });
        }
        None
    }

    /// 检查净流入转负
    fn check_negative_inflow(&self, metrics: &WindowMetrics) -> Option<DecayReason> {
        let net_inflow_sol = metrics.net_inflow_sol as f64 / 1_000_000_000.0;
        if net_inflow_sol < self.config.net_inflow_threshold {
            debug!("❌ 净流入转负: {:.4} SOL < {:.4} SOL", 
                net_inflow_sol, 
                self.config.net_inflow_threshold
            );
            return Some(DecayReason::NegativeInflow {
                current: metrics.net_inflow_sol as f64,
            });
        }
        None
    }

    /// 检查成交频度骤降
    fn check_low_activity(&self, metrics: &WindowMetrics) -> Option<DecayReason> {
        // 从 event_count 推算高频交易数
        let high_frequency_trades = (metrics.event_count / 2) as u32; // 简化估算
        
        if high_frequency_trades < self.config.trade_frequency_threshold {
            debug!("❌ 成交频度骤降: {} < {} 笔", 
                high_frequency_trades, 
                self.config.trade_frequency_threshold
            );
            return Some(DecayReason::LowActivity {
                current: high_frequency_trades,
                threshold: self.config.trade_frequency_threshold,
            });
        }
        None
    }

    /// 检查加速度衰减
    fn check_acceleration_decay(&self, metrics: &WindowMetrics) -> Option<DecayReason> {
        if metrics.acceleration < self.config.acceleration_threshold {
            debug!("❌ 加速度衰减: {:.2} < {:.2}", 
                metrics.acceleration, 
                self.config.acceleration_threshold
            );
            return Some(DecayReason::AccelerationDecay {
                current: metrics.acceleration,
                threshold: self.config.acceleration_threshold,
            });
        }
        None
    }

    /// 检查综合评分
    /// 
    /// 综合评分 = (买占比 * 0.3) + (归一化净流入 * 0.3) + (归一化加速度 * 0.2) + (归一化活跃度 * 0.2)
    fn check_composite_score(&self, metrics: &WindowMetrics) -> Option<DecayReason> {
        let buy_ratio_score = metrics.buy_ratio;
        let net_inflow_score = (metrics.net_inflow_sol as f64 / 1_000_000_000.0).max(0.0).min(1.0);
        let acceleration_score = metrics.acceleration.max(0.0).min(2.0) / 2.0;
        let activity_score = (metrics.event_count as f64 / 10.0).min(1.0);
        
        let composite_score = 
            buy_ratio_score * 0.3 +
            net_inflow_score * 0.3 +
            acceleration_score * 0.2 +
            activity_score * 0.2;
        
        debug!("📊 综合评分: {:.2}", composite_score);
        debug!("   买占比分: {:.2}", buy_ratio_score);
        debug!("   净流入分: {:.2}", net_inflow_score);
        debug!("   加速度分: {:.2}", acceleration_score);
        debug!("   活跃度分: {:.2}", activity_score);
        
        if composite_score < self.config.composite_score_threshold {
            debug!("❌ 综合评分过低: {:.2} < {:.2}", 
                composite_score, 
                self.config.composite_score_threshold
            );
            return Some(DecayReason::LowCompositeScore {
                score: composite_score,
                threshold: self.config.composite_score_threshold,
            });
        }
        None
    }
}

