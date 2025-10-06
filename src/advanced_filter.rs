/// 高级事件过滤器
/// 
/// 完整实现评估报告中提到的高级事件过滤功能
/// 
/// 核心功能:
/// 1. 金额范围过滤 - min/max SOL 金额
/// 2. Dev 交易要求 - 必须包含开发者交易
/// 3. Rug 地址黑名单 - 排除已知 rug pull 地址
/// 4. 时间窗口过滤 - 只处理特定时间范围的事件
/// 5. 交易频率过滤 - 过滤异常高频/低频交易
/// 6. 地址白名单 - 只处理白名单地址

use chrono::{DateTime, Timelike, Utc};
use log::{debug, info};
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashSet, HashMap};
use std::sync::Arc;
use parking_lot::RwLock;

use crate::types::PumpFunEvent;

/// 过滤原因
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum FilterReason {
    /// 金额过小
    AmountTooSmall { amount: f64, min: f64 },
    /// 金额过大
    AmountTooLarge { amount: f64, max: f64 },
    /// 缺少 Dev 交易
    MissingDevTrade,
    /// 黑名单地址
    BlacklistedAddress { address: Pubkey },
    /// 时间窗口外
    OutsideTimeWindow { time: DateTime<Utc> },
    /// 交易频率异常
    AbnormalFrequency { frequency: f64 },
    /// 不在白名单
    NotWhitelisted { address: Pubkey },
    /// 重复事件
    DuplicateEvent,
}


/// 高级过滤器配置
#[derive(Debug, Clone)]
pub struct AdvancedFilterConfig {
    /// 最小 SOL 金额（lamports）
    pub min_sol_amount: Option<u64>,
    /// 最大 SOL 金额（lamports）
    pub max_sol_amount: Option<u64>,
    /// 是否要求 Dev 交易
    pub require_dev_trade: bool,
    /// 是否启用黑名单
    pub enable_blacklist: bool,
    /// 是否启用白名单
    pub enable_whitelist: bool,
    /// 时间窗口开始（小时，0-23）
    pub time_window_start_hour: Option<u8>,
    /// 时间窗口结束（小时，0-23）
    pub time_window_end_hour: Option<u8>,
    /// 最小交易频率（笔/秒）
    pub min_frequency: Option<f64>,
    /// 最大交易频率（笔/秒）
    pub max_frequency: Option<f64>,
    /// 是否启用重复检测
    pub enable_duplicate_detection: bool,
    /// 重复检测窗口（秒）
    pub duplicate_window_secs: u64,
}

impl Default for AdvancedFilterConfig {
    fn default() -> Self {
        Self {
            min_sol_amount: Some(100_000_000),      // 0.1 SOL
            max_sol_amount: Some(10_000_000_000),   // 10 SOL
            require_dev_trade: true,
            enable_blacklist: true,
            enable_whitelist: false,
            time_window_start_hour: None,
            time_window_end_hour: None,
            min_frequency: None,
            max_frequency: Some(10.0),              // 最多 10 笔/秒
            enable_duplicate_detection: true,
            duplicate_window_secs: 5,
        }
    }
}

/// 高级事件过滤器
pub struct AdvancedEventFilter {
    config: AdvancedFilterConfig,
    /// 黑名单地址
    blacklist: Arc<RwLock<HashSet<Pubkey>>>,
    /// 白名单地址
    whitelist: Arc<RwLock<HashSet<Pubkey>>>,
    /// Dev 交易记录 (mint -> has_dev_trade)
    dev_trades: Arc<RwLock<HashSet<Pubkey>>>,
    /// 交易频率记录 (mint -> (count, last_reset_time))
    frequency_tracker: Arc<RwLock<HashMap<Pubkey, (u32, DateTime<Utc>)>>>,
    /// 重复事件检测 (event_hash -> timestamp)
    seen_events: Arc<RwLock<HashMap<u64, DateTime<Utc>>>>,
    /// 统计信息
    stats: Arc<RwLock<FilterStats>>,
}

/// 过滤统计
#[derive(Debug, Clone, Default)]
pub struct FilterStats {
    pub total_events: u64,
    pub passed_events: u64,
    pub filtered_events: u64,
    pub filter_reasons: HashMap<String, u64>,
}

impl AdvancedEventFilter {
    /// 创建新的高级过滤器
    pub fn new(config: AdvancedFilterConfig) -> Self {
        info!("🔍 高级事件过滤器已初始化");
        if let Some(min) = config.min_sol_amount {
            info!("   最小金额: {:.4} SOL", min as f64 / 1_000_000_000.0);
        }
        if let Some(max) = config.max_sol_amount {
            info!("   最大金额: {:.4} SOL", max as f64 / 1_000_000_000.0);
        }
        info!("   要求 Dev 交易: {}", config.require_dev_trade);
        info!("   启用黑名单: {}", config.enable_blacklist);
        info!("   启用白名单: {}", config.enable_whitelist);
        info!("   启用重复检测: {}", config.enable_duplicate_detection);
        
        Self {
            config,
            blacklist: Arc::new(RwLock::new(HashSet::new())),
            whitelist: Arc::new(RwLock::new(HashSet::new())),
            dev_trades: Arc::new(RwLock::new(HashSet::new())),
            frequency_tracker: Arc::new(RwLock::new(HashMap::new())),
            seen_events: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(FilterStats::default())),
        }
    }

    /// 使用默认配置创建
    #[allow(dead_code)]
    pub fn with_defaults() -> Self {
        Self::new(AdvancedFilterConfig::default())
    }

    /// 过滤事件
    /// 
    /// 返回 Ok(()) 如果事件通过过滤，否则返回 Err(FilterReason)
    pub fn filter(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        // 更新统计
        {
            let mut stats = self.stats.write();
            stats.total_events += 1;
        }
        
        debug!("🔍 开始过滤事件");
        debug!("   Mint: {}", event.mint);
        debug!("   类型: {:?}", event.event_type);
        
        // 1. 金额范围过滤
        if let Err(reason) = self.check_amount_range(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 2. Dev 交易要求
        if let Err(reason) = self.check_dev_trade_requirement(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 3. 黑名单检查
        if let Err(reason) = self.check_blacklist(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 4. 白名单检查
        if let Err(reason) = self.check_whitelist(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 5. 时间窗口检查
        if let Err(reason) = self.check_time_window(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 6. 交易频率检查
        if let Err(reason) = self.check_frequency(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 7. 重复事件检测
        if let Err(reason) = self.check_duplicate(event) {
            self.record_filter(reason.clone());
            return Err(reason);
        }
        
        // 通过所有过滤
        {
            let mut stats = self.stats.write();
            stats.passed_events += 1;
        }
        
        debug!("✅ 事件通过过滤");
        Ok(())
    }

    /// 检查金额范围
    fn check_amount_range(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        let amount = event.sol_amount;
        let amount_sol = amount as f64 / 1_000_000_000.0;
        
        if let Some(min) = self.config.min_sol_amount {
            if amount < min {
                debug!("❌ 金额过小: {:.4} SOL < {:.4} SOL", 
                    amount_sol, 
                    min as f64 / 1_000_000_000.0
                );
                return Err(FilterReason::AmountTooSmall {
                    amount: amount_sol,
                    min: min as f64 / 1_000_000_000.0,
                });
            }
        }
        
        if let Some(max) = self.config.max_sol_amount {
            if amount > max {
                debug!("❌ 金额过大: {:.4} SOL > {:.4} SOL", 
                    amount_sol, 
                    max as f64 / 1_000_000_000.0
                );
                return Err(FilterReason::AmountTooLarge {
                    amount: amount_sol,
                    max: max as f64 / 1_000_000_000.0,
                });
            }
        }
        
        Ok(())
    }

    /// 检查 Dev 交易要求
    fn check_dev_trade_requirement(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        if !self.config.require_dev_trade {
            return Ok(());
        }
        
        // 记录 Dev 交易
        if event.is_dev_trade {
            let mut dev_trades = self.dev_trades.write();
            dev_trades.insert(event.mint);
            debug!("✅ 记录 Dev 交易: {}", event.mint);
            return Ok(());
        }
        
        // 检查是否已有 Dev 交易
        let dev_trades = self.dev_trades.read();
        if dev_trades.contains(&event.mint) {
            return Ok(());
        }
        
        debug!("❌ 缺少 Dev 交易");
        Err(FilterReason::MissingDevTrade)
    }

    /// 检查黑名单
    fn check_blacklist(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        if !self.config.enable_blacklist {
            return Ok(());
        }
        
        let blacklist = self.blacklist.read();
        if blacklist.contains(&event.user) {
            debug!("❌ 黑名单地址: {}", event.user);
            return Err(FilterReason::BlacklistedAddress {
                address: event.user,
            });
        }
        
        Ok(())
    }

    /// 检查白名单
    fn check_whitelist(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        if !self.config.enable_whitelist {
            return Ok(());
        }
        
        let whitelist = self.whitelist.read();
        if !whitelist.contains(&event.user) {
            debug!("❌ 不在白名单: {}", event.user);
            return Err(FilterReason::NotWhitelisted {
                address: event.user,
            });
        }
        
        Ok(())
    }

    /// 检查时间窗口
    fn check_time_window(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        if self.config.time_window_start_hour.is_none() 
            && self.config.time_window_end_hour.is_none() {
            return Ok(());
        }
        
        let hour = event.timestamp.hour() as u8;
        
        if let (Some(start), Some(end)) = (
            self.config.time_window_start_hour,
            self.config.time_window_end_hour,
        ) {
            let in_window = if start <= end {
                hour >= start && hour <= end
            } else {
                // 跨午夜的窗口
                hour >= start || hour <= end
            };
            
            if !in_window {
                debug!("❌ 时间窗口外: {} 小时", hour);
                return Err(FilterReason::OutsideTimeWindow {
                    time: event.timestamp,
                });
            }
        }
        
        Ok(())
    }

    /// 检查交易频率
    fn check_frequency(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        if self.config.min_frequency.is_none() && self.config.max_frequency.is_none() {
            return Ok(());
        }
        
        let mut tracker = self.frequency_tracker.write();
        let now = Utc::now();
        
        let (count, last_reset) = tracker.entry(event.mint)
            .or_insert((0, now));
        
        // 每秒重置计数
        let elapsed = (now - *last_reset).num_milliseconds() as f64 / 1000.0;
        if elapsed >= 1.0 {
            *count = 1;
            *last_reset = now;
            return Ok(());
        }
        
        *count += 1;
        let frequency = *count as f64 / elapsed.max(0.001);
        
        if let Some(max) = self.config.max_frequency {
            if frequency > max {
                debug!("❌ 交易频率过高: {:.2} 笔/秒 > {:.2} 笔/秒", frequency, max);
                return Err(FilterReason::AbnormalFrequency { frequency });
            }
        }
        
        Ok(())
    }

    /// 检查重复事件
    fn check_duplicate(&self, event: &PumpFunEvent) -> Result<(), FilterReason> {
        if !self.config.enable_duplicate_detection {
            return Ok(());
        }
        
        // 计算事件哈希
        let event_hash = self.calculate_event_hash(event);
        
        let mut seen = self.seen_events.write();
        let now = Utc::now();
        
        // 清理过期记录
        seen.retain(|_, timestamp| {
            (now - *timestamp).num_seconds() < self.config.duplicate_window_secs as i64
        });
        
        // 检查是否重复
        if seen.contains_key(&event_hash) {
            debug!("❌ 重复事件");
            return Err(FilterReason::DuplicateEvent);
        }
        
        // 记录事件
        seen.insert(event_hash, now);
        
        Ok(())
    }

    /// 计算事件哈希
    fn calculate_event_hash(&self, event: &PumpFunEvent) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        event.mint.hash(&mut hasher);
        event.user.hash(&mut hasher);
        event.sol_amount.hash(&mut hasher);
        event.token_amount.hash(&mut hasher);
        hasher.finish()
    }

    /// 记录过滤原因
    fn record_filter(&self, reason: FilterReason) {
        let mut stats = self.stats.write();
        stats.filtered_events += 1;
        
        let reason_str = match reason {
            FilterReason::AmountTooSmall { .. } => "金额过小",
            FilterReason::AmountTooLarge { .. } => "金额过大",
            FilterReason::MissingDevTrade => "缺少Dev交易",
            FilterReason::BlacklistedAddress { .. } => "黑名单地址",
            FilterReason::OutsideTimeWindow { .. } => "时间窗口外",
            FilterReason::AbnormalFrequency { .. } => "交易频率异常",
            FilterReason::NotWhitelisted { .. } => "不在白名单",
            FilterReason::DuplicateEvent => "重复事件",
        };
        
        *stats.filter_reasons.entry(reason_str.to_string()).or_insert(0) += 1;
    }

    /// 添加黑名单地址
    #[allow(dead_code)]
    pub fn add_to_blacklist(&self, address: Pubkey) {
        let mut blacklist = self.blacklist.write();
        blacklist.insert(address);
        info!("🚫 添加黑名单地址: {}", address);
    }

    /// 添加白名单地址
    #[allow(dead_code)]
    pub fn add_to_whitelist(&self, address: Pubkey) {
        let mut whitelist = self.whitelist.write();
        whitelist.insert(address);
        info!("✅ 添加白名单地址: {}", address);
    }

    /// 获取统计信息
    #[allow(dead_code)]
    pub fn get_stats(&self) -> FilterStats {
        self.stats.read().clone()
    }

    /// 打印统计信息
    #[allow(dead_code)]
    pub fn print_stats(&self) {
        let stats = self.stats.read();
        info!("📊 过滤器统计:");
        info!("   总事件数: {}", stats.total_events);
        info!("   通过数: {}", stats.passed_events);
        info!("   过滤数: {}", stats.filtered_events);
        info!("   通过率: {:.2}%", 
            if stats.total_events > 0 {
                stats.passed_events as f64 / stats.total_events as f64 * 100.0
            } else {
                0.0
            }
        );
        
        if !stats.filter_reasons.is_empty() {
            info!("   过滤原因:");
            for (reason, count) in &stats.filter_reasons {
                info!("     {}: {} 次", reason, count);
            }
        }
    }
}

