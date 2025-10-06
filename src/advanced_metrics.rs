/// 高级指标计算
/// 
/// 完整实现评估报告中提到的高级特征计算功能
/// 
/// 核心功能:
/// 1. 曲线斜率 (curve_slope) - 价格变化速率
/// 2. 加权买压 (weighted_buy_pressure) - 考虑金额的买压
/// 3. 高频交易数 (high_frequency_trades) - 短时间内的交易数
/// 4. 价格冲击 (price_impact) - 单笔交易对价格的影响
/// 5. 流动性深度 (liquidity_depth) - 可用流动性评估
/// 6. 波动率 (volatility) - 价格波动程度

use chrono::Utc;
use log::debug;
use std::collections::VecDeque;

use crate::types::PumpFunEvent;

/// 高级指标
#[derive(Debug, Clone)]
pub struct AdvancedMetrics {
    /// 曲线斜率（价格变化速率）
    pub curve_slope: f64,
    /// 加权买压（考虑金额的买方力量）
    pub weighted_buy_pressure: f64,
    /// 高频交易数（1秒内的交易数）
    pub high_frequency_trades: u32,
    /// 平均价格冲击（单笔交易对价格的影响）
    pub avg_price_impact: f64,
    /// 最大价格冲击
    pub max_price_impact: f64,
    /// 流动性深度（归一化，0-1）
    pub liquidity_depth: f64,
    /// 价格波动率（标准差）
    pub volatility: f64,
    /// 买卖金额比（加权）
    pub weighted_buy_sell_ratio: f64,
    /// 大额交易占比
    pub large_trade_ratio: f64,
    /// 交易间隔标准差（ms）
    pub trade_interval_std: f64,
}

impl Default for AdvancedMetrics {
    fn default() -> Self {
        Self {
            curve_slope: 0.0,
            weighted_buy_pressure: 0.0,
            high_frequency_trades: 0,
            avg_price_impact: 0.0,
            max_price_impact: 0.0,
            liquidity_depth: 0.0,
            volatility: 0.0,
            weighted_buy_sell_ratio: 0.0,
            large_trade_ratio: 0.0,
            trade_interval_std: 0.0,
        }
    }
}

/// 高级指标计算器
pub struct AdvancedMetricsCalculator {
    /// 大额交易阈值（SOL）
    large_trade_threshold: f64,
    /// 高频交易时间窗口（秒）
    high_frequency_window: f64,
}

impl AdvancedMetricsCalculator {
    /// 创建新的计算器
    pub fn new(large_trade_threshold: f64, high_frequency_window: f64) -> Self {
        Self {
            large_trade_threshold,
            high_frequency_window,
        }
    }

    /// 计算高级指标
    pub fn calculate(&self, events: &VecDeque<PumpFunEvent>) -> AdvancedMetrics {
        if events.is_empty() {
            return AdvancedMetrics::default();
        }

        debug!("📊 开始计算高级指标");
        debug!("   事件数: {}", events.len());

        let mut metrics = AdvancedMetrics::default();

        // 1. 计算曲线斜率
        metrics.curve_slope = self.calculate_curve_slope(events);

        // 2. 计算加权买压
        metrics.weighted_buy_pressure = self.calculate_weighted_buy_pressure(events);

        // 3. 计算高频交易数
        metrics.high_frequency_trades = self.calculate_high_frequency_trades(events);

        // 4. 计算价格冲击
        let (avg_impact, max_impact) = self.calculate_price_impact(events);
        metrics.avg_price_impact = avg_impact;
        metrics.max_price_impact = max_impact;

        // 5. 计算流动性深度
        metrics.liquidity_depth = self.calculate_liquidity_depth(events);

        // 6. 计算波动率
        metrics.volatility = self.calculate_volatility(events);

        // 7. 计算加权买卖比
        metrics.weighted_buy_sell_ratio = self.calculate_weighted_buy_sell_ratio(events);

        // 8. 计算大额交易占比
        metrics.large_trade_ratio = self.calculate_large_trade_ratio(events);

        // 9. 计算交易间隔标准差
        metrics.trade_interval_std = self.calculate_trade_interval_std(events);

        debug!("✅ 高级指标计算完成");
        debug!("   曲线斜率: {:.6}", metrics.curve_slope);
        debug!("   加权买压: {:.4}", metrics.weighted_buy_pressure);
        debug!("   高频交易数: {}", metrics.high_frequency_trades);
        debug!("   平均价格冲击: {:.4}%", metrics.avg_price_impact * 100.0);
        debug!("   流动性深度: {:.4}", metrics.liquidity_depth);
        debug!("   波动率: {:.4}", metrics.volatility);

        metrics
    }

    /// 计算曲线斜率
    /// 
    /// 使用线性回归计算价格变化速率
    fn calculate_curve_slope(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        if events.len() < 2 {
            return 0.0;
        }

        let prices: Vec<f64> = events.iter()
            .map(|e| self.calculate_price(e))
            .collect();

        // 简单线性回归
        let n = prices.len() as f64;
        let x_mean = (n - 1.0) / 2.0;
        let y_mean = prices.iter().sum::<f64>() / n;

        let mut numerator = 0.0;
        let mut denominator = 0.0;

        for (i, price) in prices.iter().enumerate() {
            let x = i as f64;
            numerator += (x - x_mean) * (price - y_mean);
            denominator += (x - x_mean).powi(2);
        }

        if denominator == 0.0 {
            return 0.0;
        }

        numerator / denominator
    }

    /// 计算加权买压
    /// 
    /// 买压 = Σ(买入金额 * 权重) / Σ(总金额 * 权重)
    /// 权重 = 1 / (1 + 时间衰减)
    fn calculate_weighted_buy_pressure(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        if events.is_empty() {
            return 0.0;
        }

        let now = Utc::now();
        let mut weighted_buy = 0.0;
        let mut weighted_total = 0.0;

        for event in events.iter() {
            let age_secs = (now - event.timestamp).num_seconds();
            let weight = 1.0 / (1.0 + age_secs as f64 / 60.0); // 1分钟衰减

            let amount = event.sol_amount as f64;
            weighted_total += amount * weight;

            if event.is_buy {
                weighted_buy += amount * weight;
            }
        }

        if weighted_total == 0.0 {
            return 0.0;
        }

        weighted_buy / weighted_total
    }

    /// 计算高频交易数
    /// 
    /// 统计时间窗口内的交易数
    fn calculate_high_frequency_trades(&self, events: &VecDeque<PumpFunEvent>) -> u32 {
        if events.is_empty() {
            return 0;
        }

        let latest_time = events.back()
            .map(|e| e.timestamp)
            .unwrap_or_else(chrono::Utc::now);
        let cutoff_time = latest_time - chrono::Duration::try_milliseconds(
            (self.high_frequency_window * 1000.0) as i64
        ).unwrap_or(chrono::Duration::seconds(10));

        events.iter()
            .filter(|e| e.timestamp >= cutoff_time)
            .count() as u32
    }

    /// 计算价格冲击
    /// 
    /// 价格冲击 = |交易后价格 - 交易前价格| / 交易前价格
    fn calculate_price_impact(&self, events: &VecDeque<PumpFunEvent>) -> (f64, f64) {
        if events.len() < 2 {
            return (0.0, 0.0);
        }

        let mut impacts = Vec::new();

        for i in 1..events.len() {
            let prev_price = self.calculate_price(&events[i - 1]);
            let curr_price = self.calculate_price(&events[i]);

            if prev_price > 0.0 {
                let impact = ((curr_price - prev_price) / prev_price).abs();
                impacts.push(impact);
            }
        }

        if impacts.is_empty() {
            return (0.0, 0.0);
        }

        let avg = impacts.iter().sum::<f64>() / impacts.len() as f64;
        let max = impacts.iter().cloned().fold(0.0, f64::max);

        (avg, max)
    }

    /// 计算流动性深度
    /// 
    /// 基于虚拟储备量评估流动性
    fn calculate_liquidity_depth(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        if events.is_empty() {
            return 0.0;
        }

        let latest = match events.back() {
            Some(e) => e,
            None => return 0.0,
        };
        let sol_reserves = latest.virtual_sol_reserves as f64;
        let token_reserves = latest.virtual_token_reserves as f64;

        // 流动性深度 = sqrt(sol_reserves * token_reserves) / 参考值
        // 归一化到 0-1 范围
        let liquidity = (sol_reserves * token_reserves).sqrt();
        let reference = 1_000_000_000.0; // 参考值

        (liquidity / reference).min(1.0)
    }

    /// 计算波动率
    /// 
    /// 使用价格的标准差
    fn calculate_volatility(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        if events.len() < 2 {
            return 0.0;
        }

        let prices: Vec<f64> = events.iter()
            .map(|e| self.calculate_price(e))
            .collect();

        let mean = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance = prices.iter()
            .map(|p| (p - mean).powi(2))
            .sum::<f64>() / prices.len() as f64;

        variance.sqrt() / mean.max(0.0001)
    }

    /// 计算加权买卖比
    fn calculate_weighted_buy_sell_ratio(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        let mut buy_volume = 0.0;
        let mut sell_volume = 0.0;

        for event in events.iter() {
            let amount = event.sol_amount as f64;
            if event.is_buy {
                buy_volume += amount;
            } else {
                sell_volume += amount;
            }
        }

        if sell_volume == 0.0 {
            return if buy_volume > 0.0 { f64::INFINITY } else { 0.0 };
        }

        buy_volume / sell_volume
    }

    /// 计算大额交易占比
    fn calculate_large_trade_ratio(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        if events.is_empty() {
            return 0.0;
        }

        let threshold_lamports = (self.large_trade_threshold * 1_000_000_000.0) as u64;
        let large_count = events.iter()
            .filter(|e| e.sol_amount >= threshold_lamports)
            .count();

        large_count as f64 / events.len() as f64
    }

    /// 计算交易间隔标准差
    fn calculate_trade_interval_std(&self, events: &VecDeque<PumpFunEvent>) -> f64 {
        if events.len() < 2 {
            return 0.0;
        }

        let mut intervals = Vec::new();
        for i in 1..events.len() {
            let interval = (events[i].timestamp - events[i - 1].timestamp)
                .num_milliseconds();
            intervals.push(interval as f64);
        }

        if intervals.is_empty() {
            return 0.0;
        }

        let mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
        let variance = intervals.iter()
            .map(|i| (i - mean).powi(2))
            .sum::<f64>() / intervals.len() as f64;

        variance.sqrt()
    }

    /// 计算价格（基于恒定乘积公式）
    fn calculate_price(&self, event: &PumpFunEvent) -> f64 {
        let sol_reserves = event.virtual_sol_reserves as f64;
        let token_reserves = event.virtual_token_reserves as f64;

        if token_reserves == 0.0 {
            return 0.0;
        }

        sol_reserves / token_reserves
    }
}

/// 指标评分器
///
/// 将高级指标转换为 0-1 的评分
#[allow(dead_code)]
pub struct MetricsScorer;

#[allow(dead_code)]
impl MetricsScorer {
    /// 评估指标质量
    ///
    /// 返回 0-1 的综合评分
    pub fn score(metrics: &AdvancedMetrics) -> f64 {
        let mut score = 0.0;
        let mut weight_sum = 0.0;

        // 1. 曲线斜率评分（正斜率好）
        let slope_score = (metrics.curve_slope.max(0.0) / 0.001).min(1.0);
        score += slope_score * 0.15;
        weight_sum += 0.15;

        // 2. 加权买压评分（> 0.7 好）
        let pressure_score = if metrics.weighted_buy_pressure > 0.7 {
            1.0
        } else {
            metrics.weighted_buy_pressure / 0.7
        };
        score += pressure_score * 0.25;
        weight_sum += 0.25;

        // 3. 高频交易评分（> 5 笔好）
        let frequency_score = (metrics.high_frequency_trades as f64 / 5.0).min(1.0);
        score += frequency_score * 0.15;
        weight_sum += 0.15;

        // 4. 价格冲击评分（< 5% 好）
        let impact_score = if metrics.avg_price_impact < 0.05 {
            1.0
        } else {
            (0.1 - metrics.avg_price_impact) / 0.05
        }.max(0.0);
        score += impact_score * 0.15;
        weight_sum += 0.15;

        // 5. 流动性深度评分
        score += metrics.liquidity_depth * 0.15;
        weight_sum += 0.15;

        // 6. 波动率评分（适中波动好，< 0.1）
        let volatility_score = if metrics.volatility < 0.1 {
            1.0
        } else {
            (0.2 - metrics.volatility) / 0.1
        }.max(0.0);
        score += volatility_score * 0.15;
        weight_sum += 0.15;

        if weight_sum == 0.0 {
            return 0.0;
        }

        score / weight_sum
    }
}

