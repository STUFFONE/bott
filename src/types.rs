use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// 事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SniperEvent {
    /// PumpFun 交易事件
    Trade(TradeEventData),
    /// PumpFun 创建 token 事件
    CreateToken(CreateTokenEventData),
    /// PumpFun 迁移到 Raydium AMM 事件
    Migrate(MigrateEventData),
}

/// 交易事件数据 - 完整版（参考 sol-parser-sdk）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeEventData {
    // 核心交易数据
    pub mint: Pubkey,
    pub is_buy: bool,
    /// 🔥 关键字段：标识是否为 token 创建时的首次买入
    /// 参考 sol-parser-sdk/src/core/events.rs:148
    pub is_created_buy: bool,
    pub sol_amount: u64,
    pub token_amount: u64,
    pub user: Pubkey,
    pub timestamp: i64,
    pub signature: String,

    // 储备数据
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,

    // 手续费数据
    pub fee_recipient: Pubkey,
    pub fee_basis_points: u64,
    pub fee: u64,
    pub creator: Pubkey,
    pub creator_fee_basis_points: u64,
    pub creator_fee: u64,

    // 交易量追踪
    pub track_volume: bool,
    pub total_unclaimed_tokens: u64,
    pub total_claimed_tokens: u64,
    pub current_sol_volume: u64,
    pub last_update_timestamp: i64,

    // 账户信息
    pub bonding_curve: Pubkey,
    pub associated_bonding_curve: Pubkey,
    pub associated_user: Pubkey,
    pub creator_vault: Pubkey,
    pub global_volume_accumulator: Pubkey,
    pub user_volume_accumulator: Pubkey,
}

/// 创建 token 事件数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenEventData {
    pub mint: Pubkey,
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub bonding_curve: Pubkey,
    pub creator: Pubkey,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub real_token_reserves: u64,
    pub token_total_supply: u64,
    pub timestamp: i64,
    pub signature: String,
    pub associated_bonding_curve: Pubkey,
}

/// 迁移事件数据（PumpFun -> Raydium AMM）
/// 完全参考 solana-streamer 的实现
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrateEventData {
    pub mint: Pubkey,
    pub user: Pubkey,
    pub bonding_curve: Pubkey,
    pub mint_amount: u64,
    pub sol_amount: u64,
    pub pool_migration_fee: u64,
    pub timestamp: i64,
    pub pool: Pubkey,
    pub signature: String,
    // 从指令账户获取的其他字段
    pub global: Pubkey,
    pub withdraw_authority: Pubkey,
    pub associated_bonding_curve: Pubkey,
}

/// PumpFun 事件（统一格式）
#[derive(Debug, Clone)]
pub struct PumpFunEvent {
    pub mint: Pubkey,
    pub user: Pubkey,
    pub sol_amount: u64,
    pub token_amount: u64,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub timestamp: DateTime<Utc>,
    pub is_buy: bool,
    pub is_dev_trade: bool,
    pub event_type: PumpFunEventType,
}

/// PumpFun 事件类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PumpFunEventType {
    Create,
    Buy,
    Sell,
}

/// 滑窗聚合数据
#[derive(Debug, Clone)]
pub struct WindowMetrics {
    pub mint: Pubkey,
    pub net_inflow_sol: i64,
    pub buy_ratio: f64,
    pub acceleration: f64,
    pub latest_virtual_sol_reserves: u64,
    pub latest_virtual_token_reserves: u64,
    pub event_count: usize,
    // 阈值触发相关
    pub threshold_buy_amount: Option<f64>,
    // 高级指标（从聚合器传递）
    pub advanced_metrics: Option<crate::advanced_metrics::AdvancedMetrics>,
}

/// 持仓信息
#[derive(Debug, Clone)]
pub struct Position {
    pub mint: Pubkey,
    pub entry_time: DateTime<Utc>,
    pub entry_price_sol: f64,
    pub token_amount: u64,
    pub sol_invested: u64,
    pub bonding_curve: Pubkey,
    pub creator_vault: Pubkey,
    pub associated_bonding_curve: Pubkey,
    /// 最新的虚拟 SOL 储备（用于价格计算）
    pub latest_virtual_sol_reserves: u64,
    /// 最新的虚拟 Token 储备（用于价格计算）
    pub latest_virtual_token_reserves: u64,
}

/// 策略信号
#[derive(Debug, Clone, PartialEq)]
pub enum StrategySignal {
    /// 买入信号
    Buy,
    /// 卖出信号
    Sell,
    /// 持有信号
    Hold,
    /// 无操作
    None,
}

/// 曲线状态（用于滑点计算）
#[derive(Debug, Clone)]
pub struct BondingCurveState {
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
}

impl BondingCurveState {
    /// 估算买入滑点
    pub fn estimate_buy_slippage(&self, sol_amount: u64) -> f64 {
        if self.virtual_sol_reserves == 0 || self.virtual_token_reserves == 0 {
            return 100.0; // 无效状态，返回最大滑点
        }

        // 使用恒定乘积公式估算
        let k = self.virtual_sol_reserves as u128 * self.virtual_token_reserves as u128;
        let new_sol_reserves = self.virtual_sol_reserves as u128 + sol_amount as u128;
        let new_token_reserves = k / new_sol_reserves;
        let token_out = self.virtual_token_reserves as u128 - new_token_reserves;

        // 计算理想价格和实际价格
        let ideal_price = sol_amount as f64 / self.virtual_sol_reserves as f64;
        let actual_price = sol_amount as f64 / token_out as f64;

        // 滑点 = (实际价格 - 理想价格) / 理想价格 * 100
        ((actual_price - ideal_price) / ideal_price * 100.0).abs()
    }
}
