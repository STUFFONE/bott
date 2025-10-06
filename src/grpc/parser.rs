use anyhow::Result;
use base64::prelude::*;
use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;

use crate::types::{CreateTokenEventData, SniperEvent, TradeEventData, MigrateEventData};

/// PumpFun 事件类型
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum PumpFunEvent {
    Trade(TradeEventData),
    CreateToken(CreateTokenEventData),
}

/// PumpFun 事件和指令鉴别器常量（完全参考 solana-streamer）
pub mod discriminators {
    // 事件鉴别器
    pub const CREATE_TOKEN_EVENT: &[u8] =
        &[228, 69, 165, 46, 81, 203, 154, 29, 27, 114, 169, 77, 222, 235, 99, 118];
    pub const TRADE_EVENT: &[u8] =
        &[228, 69, 165, 46, 81, 203, 154, 29, 189, 219, 127, 211, 78, 230, 97, 238];
    pub const COMPLETE_PUMP_AMM_MIGRATION_EVENT: &[u8] =
        &[228, 69, 165, 46, 81, 203, 154, 29, 189, 233, 93, 185, 92, 148, 234, 148];

    // 指令鉴别器
    pub const CREATE_TOKEN_IX: &[u8] = &[24, 30, 200, 40, 5, 28, 7, 119];
    pub const BUY_IX: &[u8] = &[102, 6, 61, 18, 1, 218, 235, 234];
    pub const SELL_IX: &[u8] = &[51, 230, 133, 164, 1, 127, 131, 173];
    pub const MIGRATE_IX: &[u8] = &[155, 234, 231, 146, 236, 158, 162, 30];

    // 账户鉴别器
    #[allow(dead_code)] // 预留：用于 BondingCurve 账户识别
    pub const BONDING_CURVE_ACCOUNT: &[u8] = &[23, 183, 248, 55, 96, 216, 172, 96];
    #[allow(dead_code)] // 预留：用于 Global 账户识别
    pub const GLOBAL_ACCOUNT: &[u8] = &[167, 232, 232, 177, 200, 108, 114, 127];
}

// 保持向后兼容的常量别名
const CREATE_TOKEN_EVENT_DISCRIMINATOR: &[u8] = discriminators::CREATE_TOKEN_EVENT;
const TRADE_EVENT_DISCRIMINATOR: &[u8] = discriminators::TRADE_EVENT;

/// PumpFun Trade 事件结构（Borsh 反序列化）
/// 🔥 注意：PumpFun 事件日志本身不包含 is_created_buy 字段
/// 该字段需要在上层解析时根据交易上下文判断（参考 sol-parser-sdk）
#[derive(BorshDeserialize, Debug)]
struct PumpFunTradeEventRaw {
    mint: [u8; 32],
    sol_amount: u64,
    token_amount: u64,
    is_buy: bool,
    user: [u8; 32],
    timestamp: i64,
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
    real_sol_reserves: u64,
    real_token_reserves: u64,
    fee_recipient: [u8; 32],
    fee_basis_points: u64,
    fee: u64,
    creator: [u8; 32],
    creator_fee_basis_points: u64,
    creator_fee: u64,
    track_volume: bool,
    total_unclaimed_tokens: u64,
    total_claimed_tokens: u64,
    current_sol_volume: u64,
    last_update_timestamp: i64,
}

/// PumpFun CreateToken 事件结构（Borsh 反序列化）
/// 完全参考 solana-streamer 的实现
#[derive(BorshDeserialize, Debug)]
struct PumpFunCreateTokenEventRaw {
    name: String,
    symbol: String,
    uri: String,
    mint: [u8; 32],
    bonding_curve: [u8; 32],
    user: [u8; 32],
    creator: [u8; 32],
    timestamp: i64,
    virtual_token_reserves: u64,
    virtual_sol_reserves: u64,
    real_token_reserves: u64,
    token_total_supply: u64,
}

/// PumpFun Migrate 事件结构（Borsh 反序列化）
/// 完全参考 solana-streamer 的实现
#[derive(BorshDeserialize, Debug)]
struct PumpFunMigrateEventRaw {
    user: [u8; 32],
    mint: [u8; 32],
    mint_amount: u64,
    sol_amount: u64,
    pool_migration_fee: u64,
    bonding_curve: [u8; 32],
    timestamp: i64,
    pool: [u8; 32],
}

// PumpFun 事件大小常量（完全参考 solana-streamer）
const PUMPFUN_TRADE_EVENT_LOG_SIZE: usize = 250;
const PUMPFUN_CREATE_TOKEN_EVENT_LOG_SIZE: usize = 257;
const PUMPFUN_MIGRATE_EVENT_LOG_SIZE: usize = 160;  // 🔥 修复：应该是 160，不是 112

/// 从日志中解析 PumpFun 事件
/// 🔥 新增 is_created_buy 参数（参考 sol-parser-sdk）
pub fn parse_pumpfun_event(
    log: &str,
    signature: &str,
    slot: u64,
    is_created_buy: bool,
) -> Result<Option<SniperEvent>> {
    // 检查是否包含 Program data
    if !log.contains("Program data:") {
        return Ok(None);
    }

    // 提取 base64 编码的数据
    let parts: Vec<&str> = log.split("Program data: ").collect();
    if parts.len() < 2 {
        return Ok(None);
    }

    let data_str = parts[1].trim();
    let data = match base64::prelude::BASE64_STANDARD.decode(data_str) {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };

    // 🔥 修复: PumpFun 事件的 discriminator 是 16 字节
    // 检查数据长度（至少需要 16 字节 discriminator）
    if data.len() < 16 {
        return Ok(None);
    }

    // 提取鉴别器（前 16 字节）
    let discriminator = &data[0..16];

    // 根据鉴别器解析不同类型的事件（使用完整 16 字节比较）
    if discriminator == TRADE_EVENT_DISCRIMINATOR {
        parse_trade_event(&data[16..], signature, slot, is_created_buy)
    } else if discriminator == CREATE_TOKEN_EVENT_DISCRIMINATOR {
        parse_create_token_event(&data[16..], signature, slot)
    } else if discriminator == discriminators::COMPLETE_PUMP_AMM_MIGRATION_EVENT {
        parse_migrate_event(&data[16..], signature, slot)
    } else {
        // 不是 PumpFun 事件
        Ok(None)
    }
}

/// 解析交易事件
/// 🔥 新增 is_created_buy 参数（参考 sol-parser-sdk/src/logs/pumpfun.rs:191）
fn parse_trade_event(
    data: &[u8],
    signature: &str,
    _slot: u64,
    is_created_buy: bool,
) -> Result<Option<SniperEvent>> {
    // 检查数据大小（完全参考 solana-streamer）
    if data.len() < PUMPFUN_TRADE_EVENT_LOG_SIZE {
        return Ok(None);
    }

    let raw_event = match PumpFunTradeEventRaw::try_from_slice(&data[..PUMPFUN_TRADE_EVENT_LOG_SIZE]) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    // 使用所有字段进行日志记录
    log::debug!(
        "📊 PumpFun Trade Event: mint={}, sol_amount={}, token_amount={}, is_buy={}, fee_recipient={}, fee_basis_points={}, fee={}, creator_fee_basis_points={}, creator_fee={}, track_volume={}, total_unclaimed={}, total_claimed={}, current_volume={}, last_update={}",
        Pubkey::new_from_array(raw_event.mint),
        raw_event.sol_amount,
        raw_event.token_amount,
        raw_event.is_buy,
        Pubkey::new_from_array(raw_event.fee_recipient),
        raw_event.fee_basis_points,
        raw_event.fee,
        raw_event.creator_fee_basis_points,
        raw_event.creator_fee,
        raw_event.track_volume,
        raw_event.total_unclaimed_tokens,
        raw_event.total_claimed_tokens,
        raw_event.current_sol_volume,
        raw_event.last_update_timestamp,
    );

    let event = TradeEventData {
        // 核心交易数据
        mint: Pubkey::new_from_array(raw_event.mint),
        is_buy: raw_event.is_buy,
        is_created_buy,  // 🔥 新增字段（参考 sol-parser-sdk/src/logs/pumpfun.rs:272）
        sol_amount: raw_event.sol_amount,
        token_amount: raw_event.token_amount,
        user: Pubkey::new_from_array(raw_event.user),
        timestamp: raw_event.timestamp,
        signature: signature.to_string(),

        // 储备数据
        virtual_sol_reserves: raw_event.virtual_sol_reserves,
        virtual_token_reserves: raw_event.virtual_token_reserves,
        real_sol_reserves: raw_event.real_sol_reserves,
        real_token_reserves: raw_event.real_token_reserves,

        // 手续费数据
        fee_recipient: Pubkey::new_from_array(raw_event.fee_recipient),
        fee_basis_points: raw_event.fee_basis_points,
        fee: raw_event.fee,
        creator: Pubkey::new_from_array(raw_event.creator),
        creator_fee_basis_points: raw_event.creator_fee_basis_points,
        creator_fee: raw_event.creator_fee,

        // 交易量追踪
        track_volume: raw_event.track_volume,
        total_unclaimed_tokens: raw_event.total_unclaimed_tokens,
        total_claimed_tokens: raw_event.total_claimed_tokens,
        current_sol_volume: raw_event.current_sol_volume,
        last_update_timestamp: raw_event.last_update_timestamp,

        // 账户信息（需要从指令账户获取）
        bonding_curve: Pubkey::default(), // TODO: 从指令账户获取
        associated_bonding_curve: Pubkey::default(), // TODO: 从指令账户获取
        associated_user: Pubkey::default(), // TODO: 从指令账户获取
        creator_vault: Pubkey::default(), // TODO: 从指令账户获取
        global_volume_accumulator: Pubkey::default(), // TODO: 从指令账户获取
        user_volume_accumulator: Pubkey::default(), // TODO: 从指令账户获取
    };

    Ok(Some(SniperEvent::Trade(event)))
}

/// 解析创建 token 事件
fn parse_create_token_event(
    data: &[u8],
    signature: &str,
    _slot: u64,
) -> Result<Option<SniperEvent>> {
    // 检查数据大小（完全参考 solana-streamer）
    if data.len() < PUMPFUN_CREATE_TOKEN_EVENT_LOG_SIZE {
        return Ok(None);
    }

    let raw_event = match PumpFunCreateTokenEventRaw::try_from_slice(&data[..PUMPFUN_CREATE_TOKEN_EVENT_LOG_SIZE]) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    // 使用所有字段进行日志记录
    log::info!(
        "🆕 PumpFun CreateToken Event: name={}, symbol={}, mint={}, creator={}, user={}, bonding_curve={}",
        raw_event.name,
        raw_event.symbol,
        Pubkey::new_from_array(raw_event.mint),
        Pubkey::new_from_array(raw_event.creator),
        Pubkey::new_from_array(raw_event.user),
        Pubkey::new_from_array(raw_event.bonding_curve),
    );

    let event = CreateTokenEventData {
        mint: Pubkey::new_from_array(raw_event.mint),
        name: raw_event.name,
        symbol: raw_event.symbol,
        uri: raw_event.uri,
        bonding_curve: Pubkey::new_from_array(raw_event.bonding_curve),
        creator: Pubkey::new_from_array(raw_event.creator),
        // 从事件日志中正确获取这些字段
        virtual_sol_reserves: raw_event.virtual_sol_reserves,
        virtual_token_reserves: raw_event.virtual_token_reserves,
        real_token_reserves: raw_event.real_token_reserves,
        token_total_supply: raw_event.token_total_supply,
        timestamp: raw_event.timestamp,
        signature: signature.to_string(),
        associated_bonding_curve: Pubkey::default(), // 需要从指令账户获取
    };

    Ok(Some(SniperEvent::CreateToken(event)))
}

/// 解析迁移事件（PumpFun -> Raydium AMM）
fn parse_migrate_event(
    data: &[u8],
    signature: &str,
    _slot: u64,
) -> Result<Option<SniperEvent>> {
    // 检查数据大小（完全参考 solana-streamer）
    if data.len() < PUMPFUN_MIGRATE_EVENT_LOG_SIZE {
        return Ok(None);
    }

    let raw_event = match PumpFunMigrateEventRaw::try_from_slice(&data[..PUMPFUN_MIGRATE_EVENT_LOG_SIZE]) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    // 使用所有字段进行日志记录
    log::info!(
        "🔄 PumpFun Migrate Event: mint={}, user={}, bonding_curve={}, pool={}, mint_amount={}, sol_amount={}, pool_migration_fee={}",
        Pubkey::new_from_array(raw_event.mint),
        Pubkey::new_from_array(raw_event.user),
        Pubkey::new_from_array(raw_event.bonding_curve),
        Pubkey::new_from_array(raw_event.pool),
        raw_event.mint_amount,
        raw_event.sol_amount,
        raw_event.pool_migration_fee,
    );

    let event = MigrateEventData {
        mint: Pubkey::new_from_array(raw_event.mint),
        user: Pubkey::new_from_array(raw_event.user),
        bonding_curve: Pubkey::new_from_array(raw_event.bonding_curve),
        mint_amount: raw_event.mint_amount,
        sol_amount: raw_event.sol_amount,
        pool_migration_fee: raw_event.pool_migration_fee,
        timestamp: raw_event.timestamp,
        pool: Pubkey::new_from_array(raw_event.pool),
        signature: signature.to_string(),
        // 这些字段需要从指令账户获取（暂时使用 default）
        global: Pubkey::default(),
        withdraw_authority: Pubkey::default(),
        associated_bonding_curve: Pubkey::default(),
    };

    Ok(Some(SniperEvent::Migrate(event)))
}

/// 从交易数据中提取 PumpFun 账户信息
/// 参考 solana-streamer 的实现方式
///
/// 🔥 修复: 接收 account_indices 参数，正确映射账户索引
pub fn extract_pumpfun_accounts(
    account_keys: &[Pubkey],
    instruction_data: &[u8],
    account_indices: &[u32],  // 🔥 新增: 指令账户索引列表
) -> Option<PumpFunAccounts> {
    // 根据指令鉴别器判断指令类型
    if instruction_data.len() < 8 {
        return None;
    }

    let discriminator = &instruction_data[0..8];

    // 🔥 修复: 通过 account_indices 映射实际账户
    let get_account = |idx: usize| -> Option<Pubkey> {
        account_indices.get(idx)
            .and_then(|&index| account_keys.get(index as usize))
            .copied()
    };

    // 使用 discriminators 模块的常量
    if discriminator == discriminators::BUY_IX {
        // Buy 指令的账户布局（完全参考 sol-trade-sdk）
        // 0: global, 1: fee_recipient, 2: mint, 3: bonding_curve, 4: associated_bonding_curve,
        // 5: user_token_account, 6: payer, 7: system_program, 8: token_program,
        // 9: creator_vault ⭐, 10: event_authority, 11: program,
        // 12: global_volume_accumulator, 13: user_volume_accumulator, 14: fee_config, 15: fee_program
        if account_indices.len() >= 16 {
            return Some(PumpFunAccounts {
                mint: get_account(2)?,
                bonding_curve: get_account(3)?,
                associated_bonding_curve: get_account(4)?,
                creator_vault: get_account(9)?,
                global: get_account(0)?,
                withdraw_authority: Pubkey::default(),
                associated_user: get_account(5)?,  // 用户代币账户
                global_volume_accumulator: get_account(12)?,
                user_volume_accumulator: get_account(13)?,
            });
        }
    } else if discriminator == discriminators::SELL_IX {
        // Sell 指令的账户布局（完全参考 sol-trade-sdk）
        // 0: global, 1: fee_recipient, 2: mint, 3: bonding_curve, 4: associated_bonding_curve,
        // 5: user_token_account, 6: payer, 7: system_program, 8: creator_vault ⭐,
        // 9: token_program, 10: event_authority, 11: program, 12: fee_config, 13: fee_program
        if account_indices.len() >= 14 {
            return Some(PumpFunAccounts {
                mint: get_account(2)?,
                bonding_curve: get_account(3)?,
                associated_bonding_curve: get_account(4)?,
                creator_vault: get_account(8)?,
                global: get_account(0)?,
                withdraw_authority: Pubkey::default(),
                associated_user: get_account(5)?,  // 用户代币账户
                global_volume_accumulator: Pubkey::default(),  // Sell 没有
                user_volume_accumulator: Pubkey::default(),  // Sell 没有
            });
        }
    } else if discriminator == discriminators::CREATE_TOKEN_IX {
        // Create 指令的账户布局（完全参考 solana-streamer）
        // 0: mint, 1: mint_authority, 2: bonding_curve, 3: associated_bonding_curve,
        // 4: global, 5: mpl_token_metadata, 6: metadata, 7: user, 8: system_program,
        // 9: token_program, 10: associated_token_program, 11: rent, 12: event_authority, 13: program
        if account_indices.len() >= 11 {
            return Some(PumpFunAccounts {
                mint: get_account(0)?,
                bonding_curve: get_account(2)?,
                associated_bonding_curve: get_account(3)?,
                creator_vault: Pubkey::default(),
                global: get_account(4)?,
                withdraw_authority: Pubkey::default(),
                associated_user: Pubkey::default(),
                global_volume_accumulator: Pubkey::default(),
                user_volume_accumulator: Pubkey::default(),
            });
        }
    } else if discriminator == discriminators::MIGRATE_IX {
        // Migrate 指令的账户布局（完全参考 solana-streamer）
        // 0: global, 1: withdraw_authority, 2: mint, 3: bonding_curve, 4: associated_bonding_curve,
        // 5: pool, 6: pool_sol_token_account, 7: pool_token_account, 8: pool_lp_mint,
        // 9: pool_authority, 10: amm_config, 11: amm_observation_state, 12: amm_program,
        // 13: user, 14: user_token_account, 15: system_program, 16: token_program,
        // 17: associated_token_program, 18: event_authority, 19: program
        if account_indices.len() >= 20 {
            return Some(PumpFunAccounts {
                mint: get_account(2)?,
                bonding_curve: get_account(3)?,
                associated_bonding_curve: get_account(4)?,
                creator_vault: Pubkey::default(),
                global: get_account(0)?,
                withdraw_authority: get_account(1)?,
                associated_user: get_account(14)?,  // 用户代币账户
                global_volume_accumulator: Pubkey::default(),
                user_volume_accumulator: Pubkey::default(),
            });
        }
    }

    None
}

/// PumpFun 账户信息（完整版）
#[derive(Debug, Clone)]
pub struct PumpFunAccounts {
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub associated_bonding_curve: Pubkey,
    pub creator_vault: Pubkey,
    pub global: Pubkey,
    pub withdraw_authority: Pubkey,  // Migrate 使用
    // 新增账户
    pub associated_user: Pubkey,  // 用户代币账户
    pub global_volume_accumulator: Pubkey,  // 全局交易量累积器
    pub user_volume_accumulator: Pubkey,  // 用户交易量累积器
}

/// PumpFun BondingCurve 账户结构（完全参考 solana-streamer）
#[derive(Clone, Debug, Default, PartialEq, BorshDeserialize)]
pub struct BondingCurve {
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
    pub creator: Pubkey,
}

pub const BONDING_CURVE_SIZE: usize = 8 * 5 + 1 + 32; // 73 bytes

/// 解码 BondingCurve 账户数据
#[allow(dead_code)]
pub fn bonding_curve_decode(data: &[u8]) -> Option<BondingCurve> {
    if data.len() < BONDING_CURVE_SIZE {
        return None;
    }
    borsh::from_slice::<BondingCurve>(&data[..BONDING_CURVE_SIZE]).ok()
}

/// PumpFun Global 配置结构（完全参考 solana-streamer）
#[derive(Clone, Debug, Default, PartialEq, BorshDeserialize)]
pub struct Global {
    pub initialized: bool,
    pub authority: Pubkey,
    pub fee_recipient: Pubkey,
    pub initial_virtual_token_reserves: u64,
    pub initial_virtual_sol_reserves: u64,
    pub initial_real_token_reserves: u64,
    pub token_total_supply: u64,
    pub fee_basis_points: u64,
    pub withdraw_authority: Pubkey,
    pub enable_migrate: bool,
    pub pool_migration_fee: u64,
    pub creator_fee_basis_points: u64,
    pub fee_recipients: [Pubkey; 7],
    pub set_creator_authority: Pubkey,
    pub admin_set_creator_authority: Pubkey,
}

pub const GLOBAL_SIZE: usize = 1 + 32 * 2 + 8 * 5 + 32 + 1 + 8 * 2 + 32 * 7 + 32 * 2; // 481 bytes

/// 解码 Global 账户数据
#[allow(dead_code)]
pub fn global_decode(data: &[u8]) -> Option<Global> {
    if data.len() < GLOBAL_SIZE {
        return None;
    }
    borsh::from_slice::<Global>(&data[..GLOBAL_SIZE]).ok()
}

