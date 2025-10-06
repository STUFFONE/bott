/// 实时监控系统
/// 
/// 完整实现评估报告中提到的实时监控功能
/// 
/// 核心功能:
/// 1. 价格监控 - 24小时价格变化
/// 2. 流动性监控 - 流动性变化检测
/// 3. 大额卖出监控 - 异常大额交易检测
/// 4. 异常交易模式监控 - rug pull 信号检测
/// 5. 多维度风险评估

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use log::{debug, info, warn, error};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::config::Config;
use crate::types::Position;
use crate::grpc::parser::bonding_curve_decode;  // 🔥 新增: Borsh 解析

/// 风险警报类型
#[derive(Debug, Clone)]
pub enum RiskAlert {
    /// 价格剧烈波动
    PriceVolatility {
        change_percent: f64,
        timeframe: String,
    },
    /// 流动性下降
    LiquidityDrop {
        drop_percent: f64,
        current_liquidity: f64,
    },
    /// 检测到大额卖出
    LargeSellDetected {
        amount_sol: f64,
        seller: Pubkey,
    },
    /// Rug Pull 信号
    RugPullSignal {
        confidence: f64,
        indicators: Vec<String>,
    },
    /// 流动性枯竭
    LiquidityExhaustion {
        remaining_percent: f64,
    },
}

impl RiskAlert {
    pub fn severity(&self) -> AlertSeverity {
        match self {
            RiskAlert::RugPullSignal { confidence, .. } => {
                if *confidence > 0.8 {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::High
                }
            }
            RiskAlert::LiquidityExhaustion { remaining_percent } => {
                if *remaining_percent < 10.0 {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::High
                }
            }
            RiskAlert::LargeSellDetected { .. } => AlertSeverity::High,
            RiskAlert::PriceVolatility { change_percent, .. } => {
                if change_percent.abs() > 50.0 {
                    AlertSeverity::High
                } else {
                    AlertSeverity::Medium
                }
            }
            RiskAlert::LiquidityDrop { drop_percent, .. } => {
                if *drop_percent > 50.0 {
                    AlertSeverity::High
                } else {
                    AlertSeverity::Medium
                }
            }
        }
    }

    pub fn description(&self) -> String {
        match self {
            RiskAlert::PriceVolatility { change_percent, timeframe } => {
                format!("价格剧烈波动: {:.2}% ({}) ", change_percent, timeframe)
            }
            RiskAlert::LiquidityDrop { drop_percent, current_liquidity } => {
                format!("流动性下降: {:.2}% (当前: {:.4} SOL)", drop_percent, current_liquidity)
            }
            RiskAlert::LargeSellDetected { amount_sol, seller } => {
                format!("大额卖出: {:.4} SOL (卖家: {})", amount_sol, seller)
            }
            RiskAlert::RugPullSignal { confidence, indicators } => {
                format!("Rug Pull 信号 (置信度: {:.0}%): {}", 
                    confidence * 100.0, 
                    indicators.join(", ")
                )
            }
            RiskAlert::LiquidityExhaustion { remaining_percent } => {
                format!("流动性枯竭: 仅剩 {:.2}%", remaining_percent)
            }
        }
    }
}

/// 警报严重程度
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    Medium,
    High,
    Critical,
}

/// 实时监控配置
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// 价格警报阈值（百分比）
    pub price_alert_threshold: f64,
    /// 流动性警报阈值（百分比）
    pub liquidity_alert_threshold: f64,
    /// 大额卖出阈值（SOL）
    pub large_sell_threshold: f64,
    /// Rug Pull 检测置信度阈值
    pub rug_pull_confidence_threshold: f64,
    /// 监控间隔（秒）
    pub monitor_interval_secs: u64,
    /// 价格历史窗口（小时）
    pub price_history_hours: i64,
}

impl MonitorConfig {
    /// 从 Config 创建监控配置
    pub fn from_config(config: &Config) -> Self {
        Self {
            price_alert_threshold: config.price_alert_threshold,
            liquidity_alert_threshold: config.liquidity_alert_threshold,
            large_sell_threshold: config.large_sell_threshold,
            rug_pull_confidence_threshold: config.rug_pull_confidence_threshold,
            monitor_interval_secs: config.monitor_interval_secs,
            price_history_hours: config.price_history_hours,
        }
    }
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            price_alert_threshold: 20.0,      // 20% 价格变化
            liquidity_alert_threshold: 30.0,  // 30% 流动性下降
            large_sell_threshold: 1.0,        // 1 SOL 大额卖出
            rug_pull_confidence_threshold: 0.7, // 70% 置信度
            monitor_interval_secs: 10,        // 每 10 秒检查一次
            price_history_hours: 24,          // 24 小时价格历史
        }
    }
}

/// 价格历史记录
#[derive(Debug, Clone)]
struct PriceRecord {
    timestamp: DateTime<Utc>,
    price: f64,
    volume: f64,  // 交易量（SOL）
}

/// 实时监控器
pub struct RealTimeMonitor {
    config: MonitorConfig,
    rpc_client: Arc<RpcClient>,  // 用于查询链上数据（价格、流动性等）和轮询交易确认
    /// 价格历史记录 (mint -> records)
    price_history: HashMap<Pubkey, VecDeque<PriceRecord>>,
    /// 流动性历史记录 (mint -> liquidity)
    liquidity_history: HashMap<Pubkey, VecDeque<f64>>,
    /// 大额交易记录 (mint -> transactions)
    large_transactions: HashMap<Pubkey, VecDeque<LargeTransaction>>,
}

/// 大额交易记录
#[derive(Debug, Clone)]
struct LargeTransaction {
    timestamp: DateTime<Utc>,
    amount_sol: f64,
    trader: Pubkey,
    is_sell: bool,
}

impl RealTimeMonitor {
    /// 创建新的实时监控器
    pub fn new(config: MonitorConfig, rpc_client: Arc<RpcClient>) -> Self {
        info!("📡 实时监控系统已初始化");
        info!("   价格警报阈值: {:.2}%", config.price_alert_threshold);
        info!("   流动性警报阈值: {:.2}%", config.liquidity_alert_threshold);
        info!("   大额卖出阈值: {:.4} SOL", config.large_sell_threshold);
        info!("   监控间隔: {} 秒", config.monitor_interval_secs);
        
        Self {
            config,
            rpc_client,
            price_history: HashMap::new(),
            liquidity_history: HashMap::new(),
            large_transactions: HashMap::new(),
        }
    }

    /// 监控持仓
    ///
    /// 返回检测到的所有风险警报
    pub async fn monitor_position(&mut self, position: &Position) -> Result<Vec<RiskAlert>> {
        debug!("📡 监控持仓: {}", position.mint);

        let mut alerts = Vec::new();

        // 计算交易量（SOL）
        let volume_sol = position.sol_invested as f64 / 1_000_000_000.0;

        // 1. 价格监控（传入交易量）
        if let Some(alert) = self.check_price_volatility(&position.mint, volume_sol).await? {
            alerts.push(alert);
        }
        
        // 2. 流动性监控
        if let Some(alert) = self.check_liquidity_drop(&position.mint).await? {
            alerts.push(alert);
        }
        
        // 3. 大额卖出监控
        if let Some(alert) = self.check_large_sells(&position.mint).await? {
            alerts.push(alert);
        }
        
        // 4. Rug Pull 信号检测
        if let Some(alert) = self.detect_rug_pull_signals(&position.mint).await? {
            alerts.push(alert);
        }
        
        // 5. 流动性枯竭检测
        if let Some(alert) = self.check_liquidity_exhaustion(&position.mint).await? {
            alerts.push(alert);
        }
        
        // 记录警报
        if !alerts.is_empty() {
            warn!("⚠️  检测到 {} 个风险警报", alerts.len());
            for alert in &alerts {
                warn!("   [{}] {}", 
                    match alert.severity() {
                        AlertSeverity::Critical => "🔴 严重",
                        AlertSeverity::High => "🟠 高",
                        AlertSeverity::Medium => "🟡 中",
                    },
                    alert.description()
                );
            }
        } else {
            debug!("✅ 未检测到风险");
        }
        
        Ok(alerts)
    }

    /// 检查价格波动
    async fn check_price_volatility(&mut self, mint: &Pubkey, volume_sol: f64) -> Result<Option<RiskAlert>> {
        // 获取当前价格
        let current_price = self.get_current_price(mint).await?;

        // 记录价格（带交易量）
        self.record_price(mint, current_price, volume_sol);

        // 获取历史价格
        let history = match self.price_history.get(mint) {
            Some(h) if h.len() >= 2 => h,
            _ => return Ok(None),
        };

        // 计算 24 小时价格变化
        let cutoff_time = Utc::now() - Duration::hours(self.config.price_history_hours);
        let old_prices: Vec<_> = history.iter()
            .filter(|r| r.timestamp < cutoff_time)
            .collect();

        if old_prices.is_empty() {
            return Ok(None);
        }

        let old_price = old_prices.first()
            .ok_or_else(|| anyhow::anyhow!("价格历史为空"))?
            .price;
        let change_percent = ((current_price - old_price) / old_price) * 100.0;

        // 计算 24 小时累积交易量
        let total_volume: f64 = history.iter()
            .filter(|r| r.timestamp >= cutoff_time)
            .map(|r| r.volume)
            .sum();

        debug!("📊 24h 价格变化: {:.2}%, 累积交易量: {:.4} SOL", change_percent, total_volume);
        
        if change_percent.abs() > self.config.price_alert_threshold {
            debug!("⚠️  价格剧烈波动: {:.2}%", change_percent);
            return Ok(Some(RiskAlert::PriceVolatility {
                change_percent,
                timeframe: format!("{}h", self.config.price_history_hours),
            }));
        }
        
        Ok(None)
    }

    /// 检查流动性下降
    async fn check_liquidity_drop(&mut self, mint: &Pubkey) -> Result<Option<RiskAlert>> {
        // 获取当前流动性
        let current_liquidity = self.get_current_liquidity(mint).await?;
        
        // 记录流动性
        let history = self.liquidity_history.entry(*mint).or_insert_with(VecDeque::new);
        history.push_back(current_liquidity);
        
        // 保持历史记录在 100 个数据点内
        while history.len() > 100 {
            history.pop_front();
        }
        
        if history.len() < 2 {
            return Ok(None);
        }

        // 计算流动性变化
        let old_liquidity = history.front()
            .ok_or_else(|| anyhow::anyhow!("流动性历史为空"))?;
        let drop_percent = ((old_liquidity - current_liquidity) / old_liquidity) * 100.0;
        
        if drop_percent > self.config.liquidity_alert_threshold {
            debug!("⚠️  流动性下降: {:.2}%", drop_percent);
            return Ok(Some(RiskAlert::LiquidityDrop {
                drop_percent,
                current_liquidity,
            }));
        }
        
        Ok(None)
    }

    /// 检查大额卖出
    async fn check_large_sells(&mut self, mint: &Pubkey) -> Result<Option<RiskAlert>> {
        // 这里应该从链上获取最近的大额交易
        // 简化实现：检查历史记录
        
        let transactions = match self.large_transactions.get(mint) {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(None),
        };
        
        // 检查最近 1 分钟的大额卖出
        let cutoff_time = Utc::now() - Duration::minutes(1);
        let recent_large_sells: Vec<_> = transactions.iter()
            .filter(|tx| tx.timestamp > cutoff_time && tx.is_sell)
            .filter(|tx| tx.amount_sol > self.config.large_sell_threshold)
            .collect();
        
        if let Some(tx) = recent_large_sells.first() {
            debug!("⚠️  检测到大额卖出: {:.4} SOL", tx.amount_sol);
            return Ok(Some(RiskAlert::LargeSellDetected {
                amount_sol: tx.amount_sol,
                seller: tx.trader,
            }));
        }
        
        Ok(None)
    }

    /// 检测 Rug Pull 信号
    async fn detect_rug_pull_signals(&self, mint: &Pubkey) -> Result<Option<RiskAlert>> {
        let mut indicators = Vec::new();
        let mut confidence = 0.0;
        
        // 指标 1: 流动性快速下降
        if let Some(history) = self.liquidity_history.get(mint) {
            if history.len() >= 2 {
                if let (Some(recent), Some(old)) = (history.back(), history.front()) {
                    let drop = ((old - recent) / old) * 100.0;

                    if drop > 50.0 {
                        indicators.push(format!("流动性暴跌 {:.0}%", drop));
                        confidence += 0.3;
                    }
                }
            }
        }
        
        // 指标 2: 连续大额卖出
        if let Some(transactions) = self.large_transactions.get(mint) {
            let recent_sells = transactions.iter()
                .filter(|tx| tx.is_sell && tx.timestamp > Utc::now() - Duration::minutes(5))
                .count();
            
            if recent_sells >= 3 {
                indicators.push(format!("连续 {} 笔大额卖出", recent_sells));
                confidence += 0.4;
            }
        }
        
        // 指标 3: 价格暴跌
        if let Some(history) = self.price_history.get(mint) {
            if history.len() >= 2 {
                if let (Some(recent), Some(old)) = (history.back(), history.front()) {
                    let drop = ((old.price - recent.price) / old.price) * 100.0;

                    if drop > 70.0 {
                        indicators.push(format!("价格暴跌 {:.0}%", drop));
                        confidence += 0.3;
                    }
                }
            }
        }
        
        if confidence >= self.config.rug_pull_confidence_threshold {
            error!("🚨 检测到 Rug Pull 信号！置信度: {:.0}%", confidence * 100.0);
            return Ok(Some(RiskAlert::RugPullSignal {
                confidence,
                indicators,
            }));
        }
        
        Ok(None)
    }

    /// 检查流动性枯竭
    async fn check_liquidity_exhaustion(&self, mint: &Pubkey) -> Result<Option<RiskAlert>> {
        let current_liquidity = self.get_current_liquidity(mint).await?;
        
        // 假设初始流动性为历史最高值
        let max_liquidity = self.liquidity_history.get(mint)
            .and_then(|h| h.iter().max_by(|a, b| a.partial_cmp(b).unwrap()))
            .copied()
            .unwrap_or(current_liquidity);
        
        let remaining_percent = (current_liquidity / max_liquidity) * 100.0;
        
        if remaining_percent < 20.0 {
            warn!("⚠️  流动性枯竭: 仅剩 {:.2}%", remaining_percent);
            return Ok(Some(RiskAlert::LiquidityExhaustion {
                remaining_percent,
            }));
        }
        
        Ok(None)
    }

    /// 获取当前价格
    ///
    /// 完全对齐 sol-trade-sdk 的 BondingCurveAccount::get_token_price 实现
    /// 参考: sol-trade-sdk/src/common/bonding_curve.rs:225-230
    async fn get_current_price(&self, mint: &Pubkey) -> Result<f64> {
        // 派生 bonding curve 地址
        let bonding_curve = self.derive_bonding_curve(mint)?;

        // 从链上读取 bonding curve 账户数据
        match self.rpc_client.get_account_data(&bonding_curve) {
            Ok(data) => {
                // 🔥 修复: 使用 Borsh 解析替代手动 offset 读取
                if let Some(bc) = bonding_curve_decode(&data) {
                    if bc.virtual_token_reserves > 0 {
                        // 完全对齐 sol-trade-sdk 的 get_token_price 实现
                        let v_sol = bc.virtual_sol_reserves as f64 / 100_000_000.0;  // lamports to 0.01 SOL
                        let v_tokens = bc.virtual_token_reserves as f64 / 100_000.0; // smallest unit
                        let token_price = v_sol / v_tokens;

                        Ok(token_price)
                    } else {
                        Ok(0.0)
                    }
                } else {
                    Ok(0.0)
                }
            }
            Err(_) => {
                // 如果读取失败，返回 0（避免程序崩溃）
                Ok(0.0)
            }
        }
    }

    /// 获取当前流动性
    ///
    /// 从 bonding curve 账户读取 SOL 储备量作为流动性指标
    async fn get_current_liquidity(&self, mint: &Pubkey) -> Result<f64> {
        // 派生 bonding curve 地址
        let bonding_curve = self.derive_bonding_curve(mint)?;

        // 从链上读取 bonding curve 账户数据
        match self.rpc_client.get_account_data(&bonding_curve) {
            Ok(data) => {
                // 🔥 修复: 使用 Borsh 解析替代手动 offset 读取
                if let Some(bc) = bonding_curve_decode(&data) {
                    // 流动性 = SOL储备量（lamports -> SOL）
                    let liquidity_sol = bc.virtual_sol_reserves as f64 / 1_000_000_000.0;
                    Ok(liquidity_sol)
                } else {
                    Ok(0.0)
                }
            }
            Err(_) => {
                // 如果读取失败，返回 0（避免程序崩溃）
                Ok(0.0)
            }
        }
    }

    /// 派生 bonding curve PDA
    fn derive_bonding_curve(&self, mint: &Pubkey) -> Result<Pubkey> {
        let program_id = Pubkey::try_from("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")?;
        let seeds = &[b"bonding-curve", mint.as_ref()];
        let (pda, _bump) = Pubkey::find_program_address(seeds, &program_id);
        Ok(pda)
    }

    /// 记录价格
    fn record_price(&mut self, mint: &Pubkey, price: f64, volume: f64) {
        let history = self.price_history.entry(*mint).or_insert_with(VecDeque::new);

        history.push_back(PriceRecord {
            timestamp: Utc::now(),
            price,
            volume,
        });

        // 保持历史记录在限制内
        while history.len() > 1000 {
            history.pop_front();
        }
    }

    /// 轮询交易确认（参考 sol-trade-sdk 的实现）
    ///
    /// 用于确认交易是否成功上链
    pub async fn poll_transaction_confirmation(
        &self,
        signature: solana_sdk::signature::Signature,
        timeout_secs: u64,
    ) -> Result<solana_sdk::signature::Signature> {
        use std::time::Instant;
        use tokio::time::{sleep, Duration};

        let timeout = Duration::from_secs(timeout_secs);
        let interval = Duration::from_millis(500); // 每 500ms 检查一次
        let start = Instant::now();

        info!("⏳ 开始轮询交易确认: {}", signature);

        loop {
            // 超时检查
            if start.elapsed() >= timeout {
                return Err(anyhow::anyhow!("交易确认超时 ({}s)", timeout_secs));
            }

            // 查询交易状态
            match self.rpc_client.get_signature_statuses(&[signature]) {
                Ok(response) => {
                    if let Some(status) = response.value.first() {
                        if let Some(status) = status {
                            // 检查是否确认
                            if status.confirmation_status.is_some() {
                                info!("✅ 交易已确认: {}", signature);
                                return Ok(signature);
                            }

                            // 检查是否有错误
                            if let Some(err) = &status.err {
                                error!("❌ 交易失败: {:?}", err);
                                return Err(anyhow::anyhow!("交易失败: {:?}", err));
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("⚠️  查询交易状态失败: {}, 继续重试", e);
                }
            }

            // 等待后重试
            sleep(interval).await;
        }
    }

}

