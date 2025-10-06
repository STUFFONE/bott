use anyhow::{Context, Result};
use serde::Deserialize;
use solana_sdk::signature::Keypair;
use solana_commitment_config::CommitmentConfig;

/// 全局配置
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    // 网络配置
    pub grpc_endpoint: String,
    pub grpc_x_token: Option<String>,
    pub rpc_endpoint: String,
    pub rpc_lightspeed_endpoint: String,
    pub commitment_level: String,

    // 钱包配置
    pub wallet_private_key: String,

    // LightSpeed 配置
    pub use_lightspeed: bool,
    pub lightspeed_tip_address: String,
    pub lightspeed_tip_sol: f64,

    // SWQOS 配置
    pub swqos_enabled: bool,

    // Compute Budget 配置
    pub compute_unit_limit: u32,
    pub compute_unit_price: u64,

    // 滑窗参数
    pub window_duration_secs: u64,
    pub window_max_events: usize,

    // 策略触发条件
    pub buy_ratio_threshold: f64,
    pub net_inflow_threshold_sol: f64,
    pub acceleration_required: bool,
    pub acceleration_multiplier: f64,
    pub max_slippage_percent: f64,

    // 交易参数
    pub snipe_amount_sol: f64,
    pub slippage_percent: f64,
    pub max_positions: usize,  // 最大同时持仓数量

    // 首波狙击策略参数
    pub enable_first_wave_sniper: bool,
    pub first_wave_inflow_multiplier: f64,
    pub first_wave_buy_ratio: f64,

    // 退出策略
    pub exit_buy_ratio_threshold: f64,
    pub exit_net_inflow_threshold_sol: f64,
    pub hold_min_duration_secs: u64,
    pub hold_max_duration_secs: u64,
    pub take_profit_multiplier: f64,
    pub stop_loss_multiplier: f64,

    // 监控参数
    pub monitor_new_tokens: bool,
    pub monitor_existing_tokens: bool,
    #[allow(dead_code)]
    pub new_token_observation_secs: u64,

    // 高级过滤参数
    pub min_sol_amount: u64,
    pub max_sol_amount: u64,
    pub max_trade_frequency: f64,
    pub require_dev_trade: bool,
    pub enable_blacklist: bool,
    pub enable_whitelist: bool,
    pub enable_duplicate_detection: bool,
    pub duplicate_window_secs: u64,

    // 动态策略参数
    pub dynamic_strategy_mode: String,
    // 🔥 新增：策略模式开关（布尔值控制）
    pub enable_conservative_mode: bool,
    pub enable_balanced_mode: bool,
    pub enable_aggressive_mode: bool,
    pub enable_custom_mode: bool,
    // 保守模式参数
    pub conservative_min_buy_ratio: f64,
    pub conservative_max_slippage: f64,
    pub conservative_min_acceleration: f64,
    pub conservative_min_liquidity_depth: f64,
    pub conservative_min_high_frequency_trades: u32,
    pub conservative_max_price_impact: f64,
    pub conservative_min_composite_score: f64,
    // 平衡模式参数
    pub balanced_min_buy_ratio: f64,
    pub balanced_max_slippage: f64,
    pub balanced_min_acceleration: f64,
    pub balanced_min_liquidity_depth: f64,
    pub balanced_min_high_frequency_trades: u32,
    pub balanced_max_price_impact: f64,
    pub balanced_min_composite_score: f64,
    // 激进模式参数
    pub aggressive_min_buy_ratio: f64,
    pub aggressive_max_slippage: f64,
    pub aggressive_min_acceleration: f64,
    pub aggressive_min_liquidity_depth: f64,
    pub aggressive_min_high_frequency_trades: u32,
    pub aggressive_max_price_impact: f64,
    pub aggressive_min_composite_score: f64,
    // 🔥 自定义模式参数
    pub custom_min_buy_ratio: f64,
    pub custom_max_slippage: f64,
    pub custom_min_acceleration: f64,
    pub custom_min_liquidity_depth: f64,
    pub custom_min_high_frequency_trades: u32,
    pub custom_max_price_impact: f64,
    pub custom_min_composite_score: f64,

    // 高级指标参数
    pub large_trade_threshold_sol: f64,
    pub high_frequency_window_secs: f64,

    // 监控参数
    pub price_alert_threshold: f64,
    pub liquidity_alert_threshold: f64,
    pub large_sell_threshold: f64,
    pub rug_pull_confidence_threshold: f64,
    pub monitor_interval_secs: u64,
    pub price_history_hours: i64,

    // 阈值触发策略参数
    pub enable_threshold_trigger: bool,
    pub threshold_observation_window_secs: u64,
    pub threshold_cumulative_buy_sol: f64,
    pub threshold_buy_ratio: f64,
    pub threshold_min_buy_amount_sol: f64,
    pub threshold_max_buy_amount_sol: f64,

    // 动能衰减参数
    pub momentum_buy_ratio_threshold: f64,
    pub momentum_net_inflow_threshold: f64,
    pub momentum_activity_threshold: f64,
    pub momentum_composite_score_threshold: f64,

    // 系统参数
    pub event_queue_capacity: usize,
    pub aggregator_cleanup_interval_secs: u64,
    pub aggregator_window_ttl_secs: u64,
}

impl Config {
    /// 从环境变量加载配置
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let config = envy::from_env::<Config>()
            .context("Failed to load configuration from environment variables")?;

        config.validate()?;

        Ok(config)
    }

    /// 验证配置参数
    fn validate(&self) -> Result<()> {
        // 🔥 补充: 验证 LightSpeed 参数
        if self.lightspeed_tip_sol < 0.0 {
            anyhow::bail!("lightspeed_tip_sol must be >= 0");
        }

        // 🔥 补充: 验证 Compute Budget 参数
        if self.compute_unit_limit == 0 {
            anyhow::bail!("compute_unit_limit must be > 0");
        }

        // 🔥 补充: 验证窗口参数
        if self.window_max_events == 0 {
            anyhow::bail!("window_max_events must be > 0");
        }

        // 🔥 补充: 验证持仓参数
        if self.max_positions == 0 {
            anyhow::bail!("max_positions must be > 0");
        }

        // 验证阈值范围
        if self.buy_ratio_threshold < 0.0 || self.buy_ratio_threshold > 1.0 {
            anyhow::bail!("buy_ratio_threshold must be between 0.0 and 1.0");
        }

        if self.exit_buy_ratio_threshold < 0.0 || self.exit_buy_ratio_threshold > 1.0 {
            anyhow::bail!("exit_buy_ratio_threshold must be between 0.0 and 1.0");
        }

        // 验证金额参数
        if self.snipe_amount_sol <= 0.0 {
            anyhow::bail!("snipe_amount_sol must be greater than 0");
        }

        if self.net_inflow_threshold_sol <= 0.0 {
            anyhow::bail!("net_inflow_threshold_sol must be greater than 0");
        }

        // 验证时间参数
        if self.window_duration_secs == 0 {
            anyhow::bail!("window_duration_secs must be greater than 0");
        }

        if self.hold_min_duration_secs >= self.hold_max_duration_secs {
            anyhow::bail!("hold_min_duration_secs must be less than hold_max_duration_secs");
        }

        // 验证高级过滤参数
        if self.min_sol_amount >= self.max_sol_amount {
            anyhow::bail!("min_sol_amount must be less than max_sol_amount");
        }

        if self.max_trade_frequency <= 0.0 {
            anyhow::bail!("max_trade_frequency must be greater than 0");
        }

        // 验证动态策略模式
        if !["conservative", "balanced", "aggressive"].contains(&self.dynamic_strategy_mode.as_str()) {
            anyhow::bail!("dynamic_strategy_mode must be one of: conservative, balanced, aggressive");
        }

        // 验证动态策略参数范围
        if self.conservative_min_buy_ratio < 0.0 || self.conservative_min_buy_ratio > 1.0 {
            anyhow::bail!("conservative_min_buy_ratio must be between 0.0 and 1.0");
        }

        if self.balanced_min_buy_ratio < 0.0 || self.balanced_min_buy_ratio > 1.0 {
            anyhow::bail!("balanced_min_buy_ratio must be between 0.0 and 1.0");
        }

        if self.aggressive_min_buy_ratio < 0.0 || self.aggressive_min_buy_ratio > 1.0 {
            anyhow::bail!("aggressive_min_buy_ratio must be between 0.0 and 1.0");
        }

        // 验证首波狙击策略参数
        if self.enable_first_wave_sniper {
            if self.first_wave_inflow_multiplier < 0.0 || self.first_wave_inflow_multiplier > 1.0 {
                anyhow::bail!("first_wave_inflow_multiplier must be between 0.0 and 1.0");
            }

            if self.first_wave_buy_ratio < 0.0 || self.first_wave_buy_ratio > 1.0 {
                anyhow::bail!("first_wave_buy_ratio must be between 0.0 and 1.0");
            }
        }

        // 验证阈值触发策略参数
        if self.enable_threshold_trigger {
            if self.threshold_observation_window_secs == 0 {
                anyhow::bail!("threshold_observation_window_secs must be greater than 0");
            }

            if self.threshold_cumulative_buy_sol <= 0.0 {
                anyhow::bail!("threshold_cumulative_buy_sol must be greater than 0");
            }

            if self.threshold_buy_ratio <= 0.0 || self.threshold_buy_ratio > 1.0 {
                anyhow::bail!("threshold_buy_ratio must be between 0.0 and 1.0");
            }

            if self.threshold_min_buy_amount_sol <= 0.0 {
                anyhow::bail!("threshold_min_buy_amount_sol must be greater than 0");
            }

            if self.threshold_max_buy_amount_sol < self.threshold_min_buy_amount_sol {
                anyhow::bail!("threshold_max_buy_amount_sol must be >= threshold_min_buy_amount_sol");
            }
        }

        // 🔥 补充: 验证滑点参数
        if self.slippage_percent < 0.0 || self.slippage_percent > 100.0 {
            anyhow::bail!("slippage_percent must be between 0.0 and 100.0");
        }

        if self.max_slippage_percent < 0.0 || self.max_slippage_percent > 100.0 {
            anyhow::bail!("max_slippage_percent must be between 0.0 and 100.0");
        }

        // 🔥 补充: 验证止盈止损参数
        if self.take_profit_multiplier < 0.0 {
            anyhow::bail!("take_profit_multiplier must be >= 0.0");
        }

        if self.stop_loss_multiplier < 0.0 || self.stop_loss_multiplier > 1.0 {
            anyhow::bail!("stop_loss_multiplier must be between 0.0 and 1.0");
        }

        // 🔥 补充: 验证加速度参数
        if self.acceleration_multiplier < 0.0 {
            anyhow::bail!("acceleration_multiplier must be >= 0.0");
        }

        // 🔥 补充: 验证系统参数
        if self.event_queue_capacity == 0 {
            anyhow::bail!("event_queue_capacity must be > 0");
        }

        if self.aggregator_cleanup_interval_secs == 0 {
            anyhow::bail!("aggregator_cleanup_interval_secs must be > 0");
        }

        if self.aggregator_window_ttl_secs == 0 {
            anyhow::bail!("aggregator_window_ttl_secs must be > 0");
        }

        Ok(())
    }

    /// 获取钱包 Keypair
    pub fn get_keypair(&self) -> Result<Keypair> {
        let keypair = Keypair::from_base58_string(&self.wallet_private_key);
        Ok(keypair)
    }

    /// 获取 CommitmentConfig
    pub fn get_commitment_config(&self) -> CommitmentConfig {
        match self.commitment_level.to_lowercase().as_str() {
            "processed" => CommitmentConfig::processed(),
            "confirmed" => CommitmentConfig::confirmed(),
            "finalized" => CommitmentConfig::finalized(),
            _ => {
                log::warn!("⚠️  未知的 commitment_level: {}, 使用默认值 'confirmed'", self.commitment_level);
                CommitmentConfig::confirmed()
            }
        }
    }

    /// 获取狙击金额（lamports）
    pub fn get_snipe_amount_lamports(&self) -> u64 {
        (self.snipe_amount_sol * 1_000_000_000.0) as u64
    }

    /// 获取 LightSpeed Tip（lamports）
    pub fn get_lightspeed_tip_lamports(&self) -> u64 {
        (self.lightspeed_tip_sol * 1_000_000_000.0) as u64
    }

    /// 打印配置摘要
    pub fn print_summary(&self) {
        log::info!("=== Configuration Summary ===");
        log::info!("Network:");
        log::info!("  RPC: {}", self.rpc_endpoint);
        log::info!("  LightSpeed RPC: {}", self.rpc_lightspeed_endpoint);
        log::info!("  gRPC: {}", self.grpc_endpoint);
        log::info!("  Commitment: {}", self.commitment_level);
        log::info!("");
        log::info!("LightSpeed:");
        log::info!("  Enabled: {}", self.use_lightspeed);
        log::info!("  Tip: {} SOL", self.lightspeed_tip_sol);
        log::info!("");
        log::info!("Compute Budget:");
        log::info!("  CU Limit: {}", self.compute_unit_limit);
        log::info!("  CU Price: {}", self.compute_unit_price);
        log::info!("");
        log::info!("Strategy:");
        log::info!("  Window Duration: {}s", self.window_duration_secs);
        log::info!("  Buy Ratio Threshold: {:.2}%", self.buy_ratio_threshold * 100.0);
        log::info!("  Net Inflow Threshold: {} SOL", self.net_inflow_threshold_sol);
        log::info!("  Acceleration Required: {}", self.acceleration_required);
        log::info!("  Max Slippage: {:.1}%", self.max_slippage_percent);
        log::info!("");
        log::info!("Trading:");
        log::info!("  Snipe Amount: {} SOL", self.snipe_amount_sol);
        log::info!("  Slippage: {:.1}%", self.slippage_percent);
        log::info!("");
        log::info!("Sniper Strategies:");
        log::info!("  🚀 First Wave Sniper: {}", if self.enable_first_wave_sniper { "ENABLED" } else { "DISABLED" });
        if self.enable_first_wave_sniper {
            log::info!("     - Inflow Multiplier: {:.1}x", self.first_wave_inflow_multiplier);
            log::info!("     - Buy Ratio: {:.0}%", self.first_wave_buy_ratio * 100.0);
        }
        log::info!("  🎯 Threshold Trigger: {}", if self.enable_threshold_trigger { "ENABLED" } else { "DISABLED" });
        if self.enable_threshold_trigger {
            log::info!("     - Observation Window: {}s", self.threshold_observation_window_secs);
            log::info!("     - Cumulative Buy: {} SOL", self.threshold_cumulative_buy_sol);
            log::info!("     - Buy Ratio: {:.0}%", self.threshold_buy_ratio * 100.0);
        }
        log::info!("");
        log::info!("Exit Strategy:");
        log::info!("  Exit Buy Ratio: {:.2}%", self.exit_buy_ratio_threshold * 100.0);
        log::info!("  Exit Net Inflow: {} SOL", self.exit_net_inflow_threshold_sol);
        log::info!("  Hold Duration: {}-{}s", self.hold_min_duration_secs, self.hold_max_duration_secs);
        log::info!("  Take Profit: {}x", self.take_profit_multiplier);
        log::info!("  Stop Loss: {}x", self.stop_loss_multiplier);
        log::info!("");
        log::info!("Monitoring:");
        log::info!("  Monitor New Tokens: {}", self.monitor_new_tokens);
        log::info!("  Monitor Existing Tokens: {}", self.monitor_existing_tokens);
        log::info!("=============================");
    }
}

