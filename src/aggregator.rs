use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use log::{debug, info};
use parking_lot::RwLock;
use solana_sdk::pubkey::Pubkey;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;
use crossbeam_queue::ArrayQueue;  // 🔥 新增: 无锁队列

use crate::advanced_filter::{AdvancedEventFilter, AdvancedFilterConfig};
use crate::advanced_metrics::{AdvancedMetrics, AdvancedMetricsCalculator};
use crate::config::Config;
use crate::types::{SniperEvent, TradeEventData, WindowMetrics, PumpFunEvent, PumpFunEventType};

/// 滑窗事件
#[derive(Debug, Clone)]
struct WindowEvent {
    is_buy: bool,
    sol_amount: u64,
    timestamp: DateTime<Utc>,
}

/// 单个 mint 的滑窗数据
struct MintWindow {
    mint: Pubkey,
    events: VecDeque<WindowEvent>,
    latest_reserves: Option<ReserveState>,
    created_at: DateTime<Utc>,
    // 阈值触发相关
    cumulative_buys_sol: f64,  // 累计买入金额 (SOL)
    threshold_triggered: bool,  // 是否已触发阈值（用于防止重复触发）
}

#[derive(Debug, Clone)]
struct ReserveState {
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
}

impl MintWindow {
    fn new(mint: Pubkey) -> Self {
        Self {
            mint,
            events: VecDeque::new(),
            latest_reserves: None,
            created_at: Utc::now(),
            cumulative_buys_sol: 0.0,
            threshold_triggered: false,
        }
    }

    /// 添加事件到滑窗
    fn add_event(&mut self, event: WindowEvent, max_events: usize, window_duration: Duration, now: DateTime<Utc>) {
        // 如果是买入事件，累计买入金额
        if event.is_buy {
            self.cumulative_buys_sol += event.sol_amount as f64 / 1_000_000_000.0; // lamports -> SOL
        }

        self.events.push_back(event.clone());

        // 移除超出时间窗口的事件
        let cutoff_time = now - window_duration;
        while let Some(front) = self.events.front() {
            if front.timestamp < cutoff_time {
                self.events.pop_front();
            } else {
                break;
            }
        }

        // 限制最大事件数
        while self.events.len() > max_events {
            self.events.pop_front();
        }
    }

    /// 计算窗口指标
    fn calculate_metrics(&self) -> WindowMetrics {
        let mut buy_count = 0;
        let mut sell_count = 0;
        let mut total_buy_sol = 0u64;
        let mut total_sell_sol = 0u64;

        for event in &self.events {
            if event.is_buy {
                buy_count += 1;
                total_buy_sol += event.sol_amount;
            } else {
                sell_count += 1;
                total_sell_sol += event.sol_amount;
            }
        }

        let total_count = buy_count + sell_count;
        let buy_ratio = if total_count > 0 {
            buy_count as f64 / total_count as f64
        } else {
            0.0
        };

        let net_inflow_sol = total_buy_sol as i64 - total_sell_sol as i64;

        // 计算加速度：后半窗 vs 前半窗
        let acceleration = self.calculate_acceleration();

        let (virtual_sol, virtual_token) = if let Some(reserves) = &self.latest_reserves
        {
            (
                reserves.virtual_sol_reserves,
                reserves.virtual_token_reserves,
            )
        } else {
            (0, 0)
        };

        WindowMetrics {
            mint: self.mint,
            net_inflow_sol,
            buy_ratio,
            acceleration,
            latest_virtual_sol_reserves: virtual_sol,
            latest_virtual_token_reserves: virtual_token,
            event_count: self.events.len(),
            threshold_buy_amount: None, // 这个字段会在后面单独设置
            advanced_metrics: None, // 这个字段会在后面单独设置
        }
    }

    /// 计算加速度：后半窗净流入 / 前半窗净流入
    fn calculate_acceleration(&self) -> f64 {
        if self.events.len() < 4 {
            return 0.0;
        }

        let mid_point = self.events.len() / 2;

        let first_half_inflow: i64 = self.events.iter()
            .take(mid_point)
            .map(|e| {
                if e.is_buy {
                    e.sol_amount as i64
                } else {
                    -(e.sol_amount as i64)
                }
            })
            .sum();

        let second_half_inflow: i64 = self.events.iter()
            .skip(mid_point)
            .map(|e| {
                if e.is_buy {
                    e.sol_amount as i64
                } else {
                    -(e.sol_amount as i64)
                }
            })
            .sum();

        if first_half_inflow <= 0 {
            if second_half_inflow > 0 {
                return f64::INFINITY;
            } else {
                return 0.0;
            }
        }

        second_half_inflow as f64 / first_half_inflow as f64
    }

    /// 检查是否应该触发阈值买入
    ///
    /// 返回: (是否触发, 计算的买入金额)
    fn check_threshold_trigger(&mut self, config: &Config) -> Option<f64> {
        // 如果未启用阈值触发，直接返回
        if !config.enable_threshold_trigger {
            return None;
        }

        // 如果已经触发过，不再重复触发
        if self.threshold_triggered {
            return None;
        }

        // 检查是否还在观察窗口内
        let now = Utc::now();
        let elapsed_secs = (now - self.created_at).num_seconds() as u64;
        if elapsed_secs > config.threshold_observation_window_secs {
            return None;
        }

        // 检查累计买入是否达到阈值
        if self.cumulative_buys_sol >= config.threshold_cumulative_buy_sol {
            // 计算买入金额 = 阈值 × 比例
            let mut buy_amount = config.threshold_cumulative_buy_sol * config.threshold_buy_ratio;

            // 应用 MIN/MAX 限制
            buy_amount = buy_amount.max(config.threshold_min_buy_amount_sol);
            buy_amount = buy_amount.min(config.threshold_max_buy_amount_sol);

            // 标记已触发
            self.threshold_triggered = true;

            info!(
                "🎯 阈值触发! mint={}, 累计买入={:.4} SOL >= 阈值={:.4} SOL, 买入金额={:.4} SOL (阈值×{:.1}%)",
                self.mint,
                self.cumulative_buys_sol,
                config.threshold_cumulative_buy_sol,
                buy_amount,
                config.threshold_buy_ratio * 100.0
            );

            return Some(buy_amount);
        }

        None
    }
}

/// 滑窗聚合器（增强版）
///
/// 集成了高级事件过滤和高级指标计算
/// 使用 DashMap 实现每个 mint 独立锁，减少锁竞争
/// 使用缓存时间减少系统调用
pub struct Aggregator {
    config: Arc<Config>,
    windows: Arc<DashMap<Pubkey, Arc<RwLock<MintWindow>>>>,
    metrics_tx: mpsc::Sender<Arc<WindowMetrics>>,
    /// 高级事件过滤器
    filter: Arc<AdvancedEventFilter>,
    /// 高级指标计算器
    metrics_calculator: Arc<AdvancedMetricsCalculator>,
    /// PumpFun 事件历史（用于高级指标计算）
    event_history: Arc<DashMap<Pubkey, Arc<RwLock<VecDeque<PumpFunEvent>>>>>,
    /// 缓存的系统时间（1ms 更新一次）
    cached_time: Arc<RwLock<DateTime<Utc>>>,
}

impl Aggregator {
    pub fn new(config: Arc<Config>, metrics_tx: mpsc::Sender<Arc<WindowMetrics>>) -> Self {
        // 创建高级过滤器（从配置读取）
        let filter_config = AdvancedFilterConfig {
            min_sol_amount: Some(config.min_sol_amount),
            max_sol_amount: Some(config.max_sol_amount),
            require_dev_trade: config.require_dev_trade,
            enable_blacklist: config.enable_blacklist,
            enable_whitelist: config.enable_whitelist,
            time_window_start_hour: None,
            time_window_end_hour: None,
            min_frequency: None,
            max_frequency: Some(config.max_trade_frequency),
            enable_duplicate_detection: config.enable_duplicate_detection,
            duplicate_window_secs: config.duplicate_window_secs,
        };
        let filter = Arc::new(AdvancedEventFilter::new(filter_config));

        // 创建高级指标计算器（从配置读取）
        let metrics_calculator = Arc::new(AdvancedMetricsCalculator::new(
            config.large_trade_threshold_sol,
            config.high_frequency_window_secs,
        ));

        info!("🎯 聚合器已初始化（增强版 + DashMap + 时间缓存优化）");
        info!("   ✅ 高级事件过滤器已启用");
        info!("   ✅ 高级指标计算器已启用");
        info!("   ✅ DashMap 并发优化已启用");
        info!("   ✅ 时间缓存优化已启用");

        let cached_time = Arc::new(RwLock::new(Utc::now()));

        // 启动时间缓存更新任务（1ms 更新一次）
        let time_updater = Arc::clone(&cached_time);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(1));
            loop {
                interval.tick().await;
                *time_updater.write() = Utc::now();
            }
        });

        Self {
            config,
            windows: Arc::new(DashMap::new()),
            metrics_tx,
            filter,
            metrics_calculator,
            event_history: Arc::new(DashMap::new()),
            cached_time,
        }
    }

    /// 获取缓存的当前时间（避免频繁系统调用）
    fn now(&self) -> DateTime<Utc> {
        *self.cached_time.read()
    }

    /// 启动聚合器
    /// 🔥 优化: 从无锁队列 ArrayQueue 消费事件 + 自适应退避
    pub async fn start(&self, event_queue: Arc<ArrayQueue<SniperEvent>>) {
        info!("Aggregator started (Zero-Copy Mode + Adaptive Backoff)");

        // 🔥 优化: 自适应退避轮询（空闲时降低 CPU 占用）
        let mut backoff_delay = 100; // 初始 100μs
        const MAX_BACKOFF: u64 = 5000; // 最大 5ms
        const MIN_BACKOFF: u64 = 100;  // 最小 100μs

        loop {
            // 批量处理队列中的所有事件
            let mut events_processed = 0;
            while let Some(event) = event_queue.pop() {
                events_processed += 1;
                match event {
                    SniperEvent::Trade(trade) => {
                        self.handle_trade_event(trade).await;
                    }
                    SniperEvent::CreateToken(create) => {
                        info!("🆕 新币创建: {} ({})", create.symbol, create.mint);
                        info!("   创建者: {}", create.creator);
                        info!("   开始监控首波资金流动...");

                        // 为新 token 创建窗口（DashMap 自动处理并发）
                        self.windows.insert(
                            create.mint,
                            Arc::new(RwLock::new(MintWindow::new(create.mint)))
                        );

                        // 初始化事件历史，并添加一个 Create 类型的 PumpFunEvent
                        let timestamp = DateTime::from_timestamp(create.timestamp, 0).unwrap_or_else(Utc::now);
                        let create_event = PumpFunEvent {
                            mint: create.mint,
                            user: create.creator,
                            sol_amount: 0, // Create 事件没有交易金额
                            token_amount: create.token_total_supply,
                            virtual_sol_reserves: create.virtual_sol_reserves,
                            virtual_token_reserves: create.virtual_token_reserves,
                            timestamp,
                            is_buy: false,
                            is_dev_trade: true, // Create 事件视为 dev 操作
                            event_type: PumpFunEventType::Create, // ✅ 使用 Create 类型
                        };

                    let mut events = VecDeque::new();
                    events.push_back(create_event);
                    self.event_history.insert(
                        create.mint,
                        Arc::new(RwLock::new(events))
                    );

                    debug!("✅ Create 事件已记录: {}", create.mint);
                }
                SniperEvent::Migrate(migrate) => {
                    info!("🔄 代币已迁移到 Raydium: {}", migrate.mint);
                    info!("   Pool: {}", migrate.pool);
                    info!("   迁移金额: {} SOL, {} tokens",
                        migrate.sol_amount as f64 / 1_000_000_000.0,
                        migrate.mint_amount);
                    info!("   迁移费用: {} SOL", migrate.pool_migration_fee as f64 / 1_000_000_000.0);

                    // Migrate 事件表示 bonding curve 已完成，移除窗口和历史
                    self.windows.remove(&migrate.mint);
                    self.event_history.remove(&migrate.mint);

                    debug!("✅ Migrate 事件已处理，已移除窗口: {}", migrate.mint);
                }
            }

            // 🔥 优化: 自适应退避逻辑
            if events_processed > 0 {
                // 有事件处理，重置退避延迟
                backoff_delay = MIN_BACKOFF;
            } else {
                // 无事件，指数退避（最大 5ms）
                backoff_delay = std::cmp::min(backoff_delay * 2, MAX_BACKOFF);
            }

            tokio::time::sleep(tokio::time::Duration::from_micros(backoff_delay)).await;
        }
    }
}

    /// 处理交易事件（增强版）
    async fn handle_trade_event(&self, trade: TradeEventData) {
        // 1. 转换为 PumpFunEvent 格式
        let timestamp = DateTime::from_timestamp(trade.timestamp, 0).unwrap_or_else(Utc::now);
        let pumpfun_event = PumpFunEvent {
            mint: trade.mint,
            user: trade.user,
            sol_amount: trade.sol_amount,
            token_amount: trade.token_amount,
            virtual_sol_reserves: trade.virtual_sol_reserves,
            virtual_token_reserves: trade.virtual_token_reserves,
            timestamp,
            is_buy: trade.is_buy,
            is_dev_trade: trade.user == trade.creator,
            event_type: if trade.is_buy {
                PumpFunEventType::Buy
            } else {
                PumpFunEventType::Sell
            },
        };

        // 2. 高级事件过滤
        if let Err(reason) = self.filter.filter(&pumpfun_event) {
            debug!("❌ 事件被过滤: {:?}", reason);
            return;
        }

        // 3. 记录到事件历史（用于高级指标计算）
        {
            let events_arc = self.event_history
                .entry(trade.mint)
                .or_insert_with(|| Arc::new(RwLock::new(VecDeque::new())))
                .clone();

            let mut events = events_arc.write();
            events.push_back(pumpfun_event.clone());

            // 保留最近 100 个事件
            while events.len() > 100 {
                events.pop_front();
            }
        }

        // 4-7. 更新滑窗并计算指标（在独立作用域中，避免跨 await 持有锁）
        let metrics = {
            let window_arc = self.windows
                .entry(trade.mint)
                .or_insert_with(|| Arc::new(RwLock::new(MintWindow::new(trade.mint))))
                .clone();

            let mut window = window_arc.write();

            // 更新储备状态
            window.latest_reserves = Some(ReserveState {
                virtual_sol_reserves: trade.virtual_sol_reserves,
                virtual_token_reserves: trade.virtual_token_reserves,
            });

            // 添加事件
            let window_event = WindowEvent {
                is_buy: trade.is_buy,
                sol_amount: trade.sol_amount,
                timestamp,
            };

            let window_duration = Duration::seconds(self.config.window_duration_secs as i64);
            let now = self.now();
            window.add_event(
                window_event,
                self.config.window_max_events,
                window_duration,
                now,
            );

            // 检查阈值触发
            let _threshold_buy_amount = window.check_threshold_trigger(&self.config);

            // 计算基础指标
            let mut metrics = window.calculate_metrics();

            // 设置阈值触发信息
            metrics.threshold_buy_amount = _threshold_buy_amount;

            metrics
            // window 锁在这里自动释放
        };

        // 6. 计算高级指标并传递给 metrics
        let advanced_metrics = {
            if let Some(events_arc) = self.event_history.get(&trade.mint) {
                let events = events_arc.read();
                if events.len() >= 5 {
                    let advanced = self.metrics_calculator.calculate(&events);
                    drop(events); // 显式释放锁
                    debug!("📊 高级指标: 曲线斜率={:.6}, 加权买压={:.4}, 高频交易={}, 流动性深度={:.4}",
                        advanced.curve_slope,
                        advanced.weighted_buy_pressure,
                        advanced.high_frequency_trades,
                        advanced.liquidity_depth
                    );
                    Some(advanced)
                } else {
                    drop(events); // 显式释放锁
                    None
                }
            } else {
                None
            }
        };

        // 7. 将高级指标传递给 metrics（修复：之前是 TODO）
        let mut final_metrics = metrics;
        final_metrics.advanced_metrics = advanced_metrics;

        // 8. 发送最终指标到策略引擎（使用 Arc 避免克隆）
        if let Err(e) = self.metrics_tx.send(Arc::new(final_metrics)).await {
            log::error!("Failed to send metrics: {}", e);
        }
    }

    /// 获取高级指标（保留作为备用 API）
    #[allow(dead_code)]
    pub fn get_advanced_metrics(&self, mint: &Pubkey) -> Option<AdvancedMetrics> {
        if let Some(events_arc) = self.event_history.get(mint) {
            let events = events_arc.read();
            if events.len() >= 5 {
                return Some(self.metrics_calculator.calculate(&events));
            }
        }
        None
    }

    /// 获取指定 mint 的当前指标
    #[allow(dead_code)]
    pub fn get_metrics(&self, mint: &Pubkey) -> Option<WindowMetrics> {
        self.windows.get(mint).map(|window_arc| {
            let window = window_arc.read();
            window.calculate_metrics()
        })
    }

    /// 清理过期的窗口
    pub fn cleanup_old_windows(&self, max_age_secs: u64) {
        let cutoff_time = self.now() - Duration::seconds(max_age_secs as i64);

        // 🔥 修复: 清理过期窗口
        let mut removed_windows = 0;
        self.windows.retain(|_, window_arc| {
            let window = window_arc.read();
            let should_keep = window.created_at > cutoff_time;
            if !should_keep {
                removed_windows += 1;
            }
            should_keep
        });

        // 🔥 修复: 清理对应的事件历史（防止内存泄漏）
        let mut removed_histories = 0;
        self.event_history.retain(|mint, _| {
            let should_keep = self.windows.contains_key(mint);
            if !should_keep {
                removed_histories += 1;
            }
            should_keep
        });

        if removed_windows > 0 || removed_histories > 0 {
            info!("🧹 清理完成: 移除 {} 个窗口, {} 个事件历史", removed_windows, removed_histories);
        }
    }
}

