/// SolTrade 卖出执行器
/// 
/// 完整实现 sol-trade-sdk 的卖出逻辑，不做任何简化
/// 参考: sol-trade-sdk/examples/pumpfun_sniper_trading/src/main.rs
/// 
/// 核心功能:
/// 1. TradeSellParams 完整参数构建
/// 2. PumpFunParams::immediate_sell 逻辑
/// 3. 卖出指令构建
/// 4. 滑点控制
/// 5. Token 账户关闭选项
/// 6. 交易确认等待

use anyhow::{Context, Result};
use log::{debug, info, warn, error};
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;

// PumpFun 程序常量
#[allow(dead_code)]
const PUMPFUN_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
const PUMPFUN_GLOBAL: &str = "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf";
// 🔥 修复: FEE_RECIPIENT 应该是 62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV
// 参考: sol-trade-sdk/src/instruction/utils/pumpfun.rs:54
const PUMPFUN_FEE_RECIPIENT: &str = "62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV";
const PUMPFUN_EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";
const SYSTEM_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";  // 🔥 新增: Token-2022
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
// 🔥 修复: 对齐 sol-trade-sdk 的常量值
// 参考: sol-trade-sdk/src/instruction/utils/pumpfun.rs:106-111
const FEE_CONFIG: &str = "8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt";
const FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

// Sell 指令鉴别器 (discriminator)
const SELL_DISCRIMINATOR: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];

/// 卖出参数
/// 
/// 参考 sol-trade-sdk 的 TradeSellParams 结构
#[derive(Clone, Debug)]
pub struct SellParams {
    /// Token mint 地址
    pub mint: Pubkey,
    /// 卖出的 token 数量
    pub input_token_amount: u64,
    /// 滑点容忍度（基点，如 300 = 3%）
    pub slippage_basis_points: Option<u64>,
    /// 是否等待交易确认
    pub wait_transaction_confirmed: bool,
    /// 是否关闭 token 账户
    pub close_token_account: bool,
    /// PumpFun 特定参数
    pub pumpfun_params: PumpFunSellParams,
}

/// PumpFun 卖出特定参数
/// 
/// 参考 sol-trade-sdk 的 PumpFunParams::immediate_sell
#[derive(Clone, Debug)]
pub struct PumpFunSellParams {
    /// Bonding curve 地址
    pub bonding_curve: Pubkey,
    /// Associated bonding curve 地址
    pub associated_bonding_curve: Pubkey,
    /// Creator vault 地址
    pub creator_vault: Pubkey,
}

/// SolTrade 卖出执行器
/// 
/// 负责执行所有卖出操作，使用 sol-trade-sdk 的逻辑
pub struct SolTradeSellExecutor {
    config: Arc<Config>,
    /// RPC 客户端
    rpc_client: Arc<RpcClient>,
    /// 支付账户
    pub payer: Arc<Keypair>,
    /// PumpFun 程序地址
    pumpfun_program: Pubkey,
    /// PumpFun 全局账户
    global: Pubkey,
    /// PumpFun 费用接收账户
    fee_recipient: Pubkey,
    /// PumpFun 事件权限账户
    event_authority: Pubkey,
}

impl SolTradeSellExecutor {
    /// 创建新的 SolTrade 卖出执行器
    pub fn new(config: Arc<Config>, payer: Arc<Keypair>) -> Result<Self> {
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.rpc_endpoint.clone(),
            CommitmentConfig::confirmed(),
        ));
        
        info!("💰 SolTrade 卖出执行器已初始化");
        info!("   RPC 端点: {}", config.rpc_endpoint);
        info!("   钱包地址: {}", payer.pubkey());
        
        Ok(Self {
            config,
            rpc_client,
            payer,
            pumpfun_program: Pubkey::try_from(PUMPFUN_PROGRAM_ID)
                .context("Invalid PumpFun program ID")?,
            global: Pubkey::try_from(PUMPFUN_GLOBAL)
                .context("Invalid global account")?,
            fee_recipient: Pubkey::try_from(PUMPFUN_FEE_RECIPIENT)
                .context("Invalid fee recipient")?,
            event_authority: Pubkey::try_from(PUMPFUN_EVENT_AUTHORITY)
                .context("Invalid event authority")?,
        })
    }

    /// 执行卖出操作
    ///
    /// 参考 sol-trade-sdk 的完整流程:
    /// 1. 构建 TradeSellParams
    /// 2. 构建卖出指令
    /// 3. 发送交易（带重试机制）
    /// 4. 等待确认（如果需要）
    pub async fn execute_sell(&self, params: SellParams) -> Result<Signature> {
        info!("═══════════════════════════════════════════════════════");
        info!("💸 开始执行 SolTrade 卖出");
        info!("   Token Mint: {}", params.mint);
        info!("   卖出数量: {} tokens", params.input_token_amount);
        info!("   滑点容忍: {} bps", params.slippage_basis_points.unwrap_or(300));
        info!("   关闭账户: {}", params.close_token_account);
        info!("═══════════════════════════════════════════════════════");

        // 1. 构建卖出指令
        let instructions = self.build_sell_instructions(&params)?;

        info!("📦 卖出指令已构建，共 {} 条指令", instructions.len());

        // 2. 发送交易（带重试机制）
        let signature = self.send_transaction_with_retry(instructions).await?;

        info!("✅ 卖出交易已发送: {}", signature);

        // 3. 等待确认（如果需要）
        if params.wait_transaction_confirmed {
            let confirmed = self.wait_for_confirmation(&signature, 30).await?;

            if confirmed {
                info!("🎉 卖出交易已确认: {}", signature);
            } else {
                warn!("⚠️  卖出交易未在规定时间内确认: {}", signature);
            }
        }

        Ok(signature)
    }

    /// 构建卖出指令
    /// 
    /// 参考 sol-trade-sdk 的指令构建逻辑:
    /// 1. ComputeBudget 指令
    /// 2. PumpFun 卖出指令
    /// 3. 关闭 token 账户指令（如果需要）
    fn build_sell_instructions(&self, params: &SellParams) -> Result<Vec<Instruction>> {
        let mut instructions = Vec::new();
        let payer = self.payer.pubkey();

        info!("🏗️  开始构建 PumpFun 卖出指令");
        debug!("   Bonding Curve: {}", params.pumpfun_params.bonding_curve);
        debug!("   Associated Bonding Curve: {}", params.pumpfun_params.associated_bonding_curve);
        debug!("   Creator Vault: {}", params.pumpfun_params.creator_vault);

        // 2. 构建 PumpFun 卖出指令
        debug!("🏗️  构建 PumpFun 卖出指令");
        
        // 获取用户 token 账户地址
        let user_token_account = Self::get_associated_token_address(&payer, &params.mint);
        debug!("   用户 Token 账户: {}", user_token_account);
        
        // 计算最小输出金额（考虑滑点）
        let slippage_bps = params.slippage_basis_points.unwrap_or(300); // 默认 3%
        let min_sol_output = self.calculate_min_sol_output(
            params.input_token_amount,
            slippage_bps,
            &params.pumpfun_params,
        )?;
        
        debug!("   最小输出: {} lamports (滑点 {} bps)", min_sol_output, slippage_bps);
        
        // 构建指令数据
        // 格式: [discriminator(8), amount(8), min_sol_output(8)]
        let mut instruction_data = Vec::with_capacity(24);
        instruction_data.extend_from_slice(&SELL_DISCRIMINATOR);
        instruction_data.extend_from_slice(&params.input_token_amount.to_le_bytes());
        instruction_data.extend_from_slice(&min_sol_output.to_le_bytes());
        
        // 构建账户列表（完全参考 sol-trade-sdk 的顺序）
        let accounts = vec![
            AccountMeta::new_readonly(self.global, false),                          // 0: global
            AccountMeta::new(self.fee_recipient, false),                            // 1: fee_recipient
            AccountMeta::new_readonly(params.mint, false),                          // 2: mint
            AccountMeta::new(params.pumpfun_params.bonding_curve, false),           // 3: bonding_curve
            AccountMeta::new(params.pumpfun_params.associated_bonding_curve, false), // 4: associated_bonding_curve
            AccountMeta::new(user_token_account, false),                            // 5: user_token_account
            AccountMeta::new(payer, true),                                          // 6: payer (signer)
            AccountMeta::new_readonly(Pubkey::try_from(SYSTEM_PROGRAM).unwrap(), false), // 7: system_program
            AccountMeta::new(params.pumpfun_params.creator_vault, false),           // 8: creator_vault ⭐
            AccountMeta::new_readonly(Pubkey::try_from(SYSTEM_TOKEN_PROGRAM).unwrap(), false), // 9: token_program ⭐
            AccountMeta::new_readonly(self.event_authority, false),                 // 10: event_authority
            AccountMeta::new_readonly(self.pumpfun_program, false),                 // 11: pumpfun_program
            AccountMeta::new_readonly(Pubkey::try_from(FEE_CONFIG).unwrap(), false), // 12: fee_config ⭐
            AccountMeta::new_readonly(Pubkey::try_from(FEE_PROGRAM).unwrap(), false), // 13: fee_program ⭐
        ];

        // 🔥 排障日志: 打印关键账户表摘要
        debug!("📋 PumpFun 卖出账户表摘要 (14 accounts):");
        debug!("   [0] global: {} (readonly)", self.global);
        debug!("   [1] fee_recipient: {} (writable)", self.fee_recipient);
        debug!("   [8] creator_vault: {} (writable) ⭐", params.pumpfun_params.creator_vault);
        debug!("   [9] token_program: {} (readonly, Token v3) ⭐",
            Pubkey::try_from(SYSTEM_TOKEN_PROGRAM).unwrap()
        );
        debug!("   [12] fee_config: {} (readonly) ⭐", Pubkey::try_from(FEE_CONFIG).unwrap());
        debug!("   [13] fee_program: {} (readonly) ⭐", Pubkey::try_from(FEE_PROGRAM).unwrap());

        instructions.push(Instruction {
            program_id: self.pumpfun_program,
            accounts,
            data: instruction_data,
        });

        // 3. 关闭 token 账户指令（如果需要）
        if params.close_token_account {
            debug!("🗑️  添加关闭 Token 账户指令");
            instructions.push(self.build_close_account_instruction(&user_token_account, &params.mint)?);
        }

        // 1. 添加计算预算指令（最后插入到开头，完全参考 lightspeed-examples 的 unshift 逻辑）
        debug!("📊 添加 ComputeBudget 指令");
        debug!("   Compute Unit Limit: {}", self.config.compute_unit_limit);
        debug!("   Compute Unit Price: {}", self.config.compute_unit_price);

        instructions.insert(0, ComputeBudgetInstruction::set_compute_unit_price(
            self.config.compute_unit_price,
        ));
        instructions.insert(0, ComputeBudgetInstruction::set_compute_unit_limit(
            self.config.compute_unit_limit,
        ));

        Ok(instructions)
    }

    /// 计算最小输出金额（考虑滑点）
    ///
    /// 完全对齐 sol-trade-sdk 的 BondingCurveAccount::get_sell_price 实现
    fn calculate_min_sol_output(
        &self,
        token_amount: u64,
        slippage_bps: u64,
        params: &PumpFunSellParams,
    ) -> Result<u64> {
        // 尝试从 bonding curve 读取真实储备量
        match self.get_bonding_curve_reserves(&params.bonding_curve) {
            Ok((virtual_token_reserves, virtual_sol_reserves)) => {
                if virtual_token_reserves > 0 && virtual_sol_reserves > 0 {
                    // 完全对齐 sol-trade-sdk 的 get_sell_price 实现
                    // 🔥 修复: 使用正确的费率 FEE_BASIS_POINTS=95 + CREATOR_FEE=30
                    // 参考: sol-trade-sdk/src/common/bonding_curve.rs:152-169

                    const FEE_BASIS_POINTS: u128 = 95;     // 0.95%
                    const CREATOR_FEE: u128 = 30;          // 0.30%
                    let total_fee_basis_points = FEE_BASIS_POINTS + CREATOR_FEE;  // 1.25%

                    // Calculate the proportional amount of virtual sol reserves to be received using u128
                    let n: u128 = ((token_amount as u128) * (virtual_sol_reserves as u128))
                        / ((virtual_token_reserves as u128) + (token_amount as u128));

                    // Calculate the fee amount in the same units
                    let a: u128 = (n * total_fee_basis_points) / 10000;

                    // 🔥 修复: 安全转换，避免溢出
                    // Return the net amount after deducting the fee
                    let estimated_output_u128 = n.saturating_sub(a);
                    let estimated_output = estimated_output_u128.min(u64::MAX as u128) as u64;

                    // 应用滑点（使用 u128 计算后再转换）
                    let slippage_multiplier = 10000 - slippage_bps;
                    let min_output_u128 = estimated_output_u128
                        .saturating_mul(slippage_multiplier as u128)
                        .checked_div(10000)
                        .unwrap_or(0);
                    let min_output = min_output_u128.min(u64::MAX as u128) as u64;

                    debug!("💱 sol-trade-sdk get_sell_price: {} tokens -> {} SOL (after 1.25% fee)",
                        token_amount,
                        estimated_output as f64 / 1_000_000_000.0
                    );
                    debug!("   应用 {}% 滑点 -> min {} SOL",
                        slippage_bps as f64 / 100.0,
                        min_output as f64 / 1_000_000_000.0
                    );

                    return Ok(min_output);
                }
            }
            Err(e) => {
                warn!("⚠️  无法读取 bonding curve 储备量: {}, 使用保守估计", e);
            }
        }

        // Fallback: 保守估计（仅在链上读取失败时）
        let estimated_output = token_amount;
        let slippage_multiplier = 10000 - slippage_bps;
        // 🔥 修复: 安全计算，避免溢出
        let min_output_u128 = (estimated_output as u128)
            .saturating_mul(slippage_multiplier as u128)
            .checked_div(10000)
            .unwrap_or(0);
        let min_output = min_output_u128.min(u64::MAX as u128) as u64;

        debug!("💱 保守估计: {} tokens -> min {} SOL with {}% slippage",
            token_amount,
            min_output as f64 / 1_000_000_000.0,
            slippage_bps as f64 / 100.0
        );

        Ok(min_output)
    }

    /// 从 bonding curve 账户读取储备量
    fn get_bonding_curve_reserves(&self, bonding_curve: &Pubkey) -> Result<(u64, u64)> {
        let data = self.rpc_client.get_account_data(bonding_curve)
            .context("读取 bonding curve 账户失败")?;

        if data.len() >= 24 {
            // PumpFun bonding curve 数据格式:
            // - virtual_token_reserves: u64 (offset 8)
            // - virtual_sol_reserves: u64 (offset 16)
            let virtual_token_reserves = u64::from_le_bytes(
                data[8..16].try_into().unwrap_or([0u8; 8])
            );
            let virtual_sol_reserves = u64::from_le_bytes(
                data[16..24].try_into().unwrap_or([0u8; 8])
            );

            Ok((virtual_token_reserves, virtual_sol_reserves))
        } else {
            Err(anyhow::anyhow!("Bonding curve 数据长度不足"))
        }
    }

    /// 构建关闭账户指令
    /// 🔥 修复: 支持 Token-2022
    fn build_close_account_instruction(&self, token_account: &Pubkey, mint: &Pubkey) -> Result<Instruction> {
        // 🔥 新增: 检测 token program（支持 Token-2022）
        let token_program = self.detect_token_program(mint)?;

        let accounts = vec![
            AccountMeta::new(*token_account, false),
            AccountMeta::new(self.payer.pubkey(), false),
            AccountMeta::new_readonly(self.payer.pubkey(), true),
        ];

        let instruction = Instruction {
            program_id: token_program,  // 🔥 使用动态检测的 token program
            accounts,
            data: vec![9], // CloseAccount 指令索引
        };

        Ok(instruction)
    }

    /// 🔥 新增: 检测 mint 的 token program（支持 Token-2022）
    fn detect_token_program(&self, mint: &Pubkey) -> Result<Pubkey> {
        // 读取 mint 账户
        let account = self.rpc_client.get_account(mint)
            .context("读取 mint 账户失败")?;

        // 检查 owner（即 token program）
        let token_program = account.owner;

        let token_2022 = Pubkey::try_from(TOKEN_2022_PROGRAM)?;
        let token_v3 = Pubkey::try_from(SYSTEM_TOKEN_PROGRAM)?;

        if token_program == token_2022 {
            debug!("🔍 检测到 Token-2022: {}", mint);
            Ok(token_2022)
        } else if token_program == token_v3 {
            debug!("🔍 检测到 Token v3: {}", mint);
            Ok(token_v3)
        } else {
            warn!("⚠️  未知 token program: {}", token_program);
            Ok(token_v3) // fallback to v3
        }
    }

    /// 获取 Associated Token Address
    fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
        let token_program_id = Pubkey::try_from("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
            .expect("Invalid TOKEN_PROGRAM_ID");

        let associated_token_program_id = Pubkey::try_from("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
            .expect("Invalid ASSOCIATED_TOKEN_PROGRAM_ID");

        Pubkey::find_program_address(
            &[
                wallet.as_ref(),
                token_program_id.as_ref(),
                mint.as_ref(),
            ],
            &associated_token_program_id,
        )
        .0
    }

    /// 发送交易（带重试机制）
    ///
    /// 最多重试 3 次
    async fn send_transaction_with_retry(&self, instructions: Vec<Instruction>) -> Result<Signature> {
        let max_attempts = 3;

        for attempt in 1..=max_attempts {
            info!("📤 发送卖出交易 (尝试 {}/{})", attempt, max_attempts);

            match self.send_transaction(instructions.clone()).await {
                Ok(signature) => {
                    if attempt > 1 {
                        info!("✅ 卖出交易发送成功 (第 {} 次尝试)", attempt);
                    }
                    return Ok(signature);
                }
                Err(e) => {
                    if attempt < max_attempts {
                        warn!("⚠️  卖出交易发送失败 (尝试 {}/{}): {}", attempt, max_attempts, e);
                        warn!("   {}ms 后重试...", 100 * attempt);
                        tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempt as u64)).await;
                    } else {
                        error!("❌ 卖出交易发送失败，已达最大重试次数: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("卖出交易发送失败，已达最大重试次数"))
    }

    /// 发送交易
    ///
    /// 参考 sol-trade-sdk 的交易发送逻辑
    async fn send_transaction(&self, instructions: Vec<Instruction>) -> Result<Signature> {
        info!("📤 准备发送卖出交易");

        // 获取最新 blockhash
        let recent_blockhash = self.rpc_client.get_latest_blockhash()
            .context("获取 blockhash 失败")?;

        // 构建交易
        let mut transaction = Transaction::new_with_payer(
            &instructions,
            Some(&self.payer.pubkey()),
        );
        transaction.sign(&[&*self.payer], recent_blockhash);

        // 发送交易
        let signature = self.rpc_client.send_transaction(&transaction)
            .context("发送交易失败")?;

        info!("✅ 卖出交易已发送: {}", signature);
        Ok(signature)
    }

    /// 等待交易确认
    ///
    /// 参考 sol-trade-sdk 的确认等待逻辑
    async fn wait_for_confirmation(
        &self,
        signature: &Signature,
        max_wait_seconds: u64,
    ) -> Result<bool> {
        info!("⏳ 等待卖出交易确认: {}", signature);
        info!("   最大等待时间: {} 秒", max_wait_seconds);

        let start_time = Instant::now();
        let max_wait = Duration::from_secs(max_wait_seconds);

        while start_time.elapsed() < max_wait {
            match self.rpc_client.get_signature_status(signature) {
                Ok(Some(status)) => {
                    match status {
                        Ok(_) => {
                            let elapsed = start_time.elapsed().as_secs();
                            info!("✅ 卖出交易已确认 (耗时 {} 秒)", elapsed);
                            return Ok(true);
                        }
                        Err(e) => {
                            error!("❌ 卖出交易失败: {:?}", e);
                            return Ok(false);
                        }
                    }
                }
                Ok(None) => {
                    debug!("⏳ 交易尚未确认，继续等待...");
                }
                Err(e) => {
                    warn!("⚠️  查询交易状态失败: {:?}", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        warn!("⏰ 卖出交易确认超时 ({} 秒)", max_wait_seconds);
        Ok(false)
    }

    /// 获取 token 账户余额
    pub async fn get_token_balance(&self, mint: &Pubkey) -> Result<u64> {
        let token_account = Self::get_associated_token_address(&self.payer.pubkey(), mint);

        match self.rpc_client.get_token_account_balance(&token_account) {
            Ok(balance) => {
                let amount = balance.amount.parse::<u64>()
                    .context("解析 token 余额失败")?;
                Ok(amount)
            }
            Err(e) => {
                warn!("获取 token 余额失败: {:?}", e);
                Ok(0)
            }
        }
    }

}


