/// LightSpeed 买入执行器
/// 
/// 完整实现 lightspeed-examples 的逻辑，不做任何简化
/// 参考: lightspeed-examples/src/utils.ts
/// 
/// 核心功能:
/// 1. LightSpeed RPC 端点连接
/// 2. LightSpeed tip 机制 (TIPS_VIBE_STATION + TIPS_VIBE_FEE)
/// 3. ComputeBudget 优先级设置
/// 4. PumpFun 买入指令构建
/// 5. 交易重试机制 (sendTxWithRetries)
/// 6. 交易状态监控 (monitorTransactionStatus)
/// 7. 余额检查 (checkBalanceForOperations)

use anyhow::{Context, Result};
use log::{debug, info, warn, error};
use solana_client::rpc_client::RpcClient;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
    message::{VersionedMessage, v0},
    transaction::VersionedTransaction,
};
use solana_system_interface::instruction::transfer;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::swqos::{SwqosConfig, MultiSwqosManager};

// PumpFun 程序常量
#[allow(dead_code)]
const PUMPFUN_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
#[allow(dead_code)]
const PUMPFUN_GLOBAL: &str = "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf";
#[allow(dead_code)]
// 🔥 修复: FEE_RECIPIENT 应该是 62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV
// 参考: sol-trade-sdk/src/instruction/utils/pumpfun.rs:54
const PUMPFUN_FEE_RECIPIENT: &str = "62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV";
#[allow(dead_code)]
const PUMPFUN_EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";
#[allow(dead_code)]
const SYSTEM_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";  // 🔥 新增: Token-2022
#[allow(dead_code)]
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
// 🔥 修复: 对齐 sol-trade-sdk 的常量值
// 参考: sol-trade-sdk/src/instruction/utils/pumpfun.rs:106-111
const GLOBAL_VOLUME_ACCUMULATOR: &str = "Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y";
const FEE_CONFIG: &str = "8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt";
const FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

// Buy 指令鉴别器 (discriminator)
#[allow(dead_code)]
const BUY_DISCRIMINATOR: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];

/// LightSpeed 买入执行器（集成 SWQOS）
///
/// 负责执行所有买入操作，支持：
/// - LightSpeed 优先级 RPC
/// - SWQOS 多服务商并行发送（田忌赛马）
/// - 自动 fallback 机制
#[allow(dead_code)]
pub struct LightSpeedBuyExecutor {
    config: Arc<Config>,
    /// 普通 RPC 客户端（用于查询）
    rpc_client: Arc<RpcClient>,
    /// LightSpeed RPC 客户端（用于发送交易，仅当启用时创建）
    lightspeed_rpc: Option<Arc<RpcClient>>,
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
    /// SWQOS 管理器（可选）
    swqos_manager: Option<Arc<MultiSwqosManager>>,
}

#[allow(dead_code)]
impl LightSpeedBuyExecutor {
    /// 创建新的 LightSpeed 买入执行器（集成 SWQOS）
    pub fn new(config: Arc<Config>, payer: Arc<Keypair>) -> Result<Self> {
        let commitment = config.get_commitment_config();

        // 普通 RPC 客户端
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.rpc_endpoint.clone(),
            commitment.clone(),
        ));

        // LightSpeed RPC 客户端（仅当启用时创建）
        let lightspeed_rpc = if config.use_lightspeed {
            info!("✅ LightSpeed 已启用，创建 LightSpeed RPC 客户端");
            Some(Arc::new(RpcClient::new_with_commitment(
                config.rpc_lightspeed_endpoint.clone(),
                commitment.clone(),
            )))
        } else {
            info!("ℹ️  LightSpeed 已禁用");
            None
        };

        // 初始化 SWQOS 管理器（如果启用）
        let swqos_manager = if config.swqos_enabled {
            match SwqosConfig::from_env() {
                Ok(swqos_config) => {
                    match MultiSwqosManager::new(swqos_config) {
                        Ok(manager) => {
                            info!("✅ SWQOS 管理器已初始化");
                            Some(Arc::new(manager))
                        }
                        Err(e) => {
                            warn!("⚠️  SWQOS 初始化失败: {}, 将只使用 LightSpeed", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    warn!("⚠️  SWQOS 配置加载失败: {}, 将只使用 LightSpeed", e);
                    None
                }
            }
        } else {
            info!("ℹ️  SWQOS 已禁用，只使用 LightSpeed");
            None
        };

        info!("🚀 LightSpeed 买入执行器已初始化");
        info!("   RPC 端点: {}", config.rpc_endpoint);
        info!("   Commitment Level: {}", config.commitment_level);
        if config.use_lightspeed {
            info!("   LightSpeed RPC: {}", config.rpc_lightspeed_endpoint);
        }
        info!("   钱包地址: {}", payer.pubkey());
        if swqos_manager.is_some() {
            info!("   SWQOS: 已启用（田忌赛马模式）");
        }

        Ok(Self {
            config,
            rpc_client,
            lightspeed_rpc,
            payer,
            pumpfun_program: Pubkey::try_from(PUMPFUN_PROGRAM_ID)
                .context("Invalid PumpFun program ID")?,
            global: Pubkey::try_from(PUMPFUN_GLOBAL)
                .context("Invalid global account")?,
            fee_recipient: Pubkey::try_from(PUMPFUN_FEE_RECIPIENT)
                .context("Invalid fee recipient")?,
            event_authority: Pubkey::try_from(PUMPFUN_EVENT_AUTHORITY)
                .context("Invalid event authority")?,
            swqos_manager,
        })
    }

    /// 执行买入操作（集成 SWQOS）
    ///
    /// 流程:
    /// 1. checkBalanceForOperations - 检查余额（包含 tip）
    /// 2. 🔥 从链上读取最新 bonding_curve 数据（real_token_reserves + virtual_sol_reserves）
    /// 3. 构建交易指令（包含 SWQOS tips）
    /// 4. 构建 VersionedTransaction
    /// 5. **优先使用 SWQOS 田忌赛马发送**
    /// 6. SWQOS 失败则 fallback 到 LightSpeed
    /// 7. monitorTransactionStatus - 监控交易状态
    ///
    /// 🔥 修复: 移除 virtual_token_reserves/virtual_sol_reserves 参数，改为从链上读取
    pub async fn execute_buy(
        &self,
        mint: &Pubkey,
        bonding_curve: &Pubkey,
        associated_bonding_curve: &Pubkey,
        sol_amount: u64,
    ) -> Result<Signature> {
        info!("═══════════════════════════════════════════════════════");
        info!("🎯 开始执行买入交易");
        info!("   Token Mint: {}", mint);
        info!("   Bonding Curve: {}", bonding_curve);
        info!("   购买金额: {} SOL", sol_amount as f64 / 1_000_000_000.0);
        info!("═══════════════════════════════════════════════════════");

        // 🔥 修复: 从链上读取最新 bonding_curve 数据（获取 real_token_reserves + virtual_token_reserves）
        //
        // 📝 设计说明：为何不使用聚合器 metrics 的 reserves？
        //    1. metrics 缺少 real_token_reserves（事件有但聚合器未保存）
        //    2. 计算需要 real_token_reserves 做 min 操作确保不超买
        //    3. 聚合器数据可能有网络延迟（~10-50ms）
        //    4. 链上读取是唯一可信源，确保计算准确性
        //    5. 延迟成本：~10-20ms RPC 调用，对极限狙击影响可控
        //
        // ⚠️ 如需优化：可将 real_token_reserves 加入 WindowMetrics，并添加时间戳校验
        let (real_token_reserves, virtual_token_reserves, virtual_sol_reserves) = {
            use crate::grpc::parser::bonding_curve_decode;

            let data = self.rpc_client.get_account_data(bonding_curve)
                .context("读取 bonding curve 账户失败")?;

            let bc = bonding_curve_decode(&data)
                .ok_or_else(|| anyhow::anyhow!("解码 bonding curve 失败"))?;

            info!("📊 链上储备数据:");
            info!("   real_token_reserves: {}", bc.real_token_reserves);
            info!("   virtual_token_reserves: {}", bc.virtual_token_reserves);
            info!("   virtual_sol_reserves: {}", bc.virtual_sol_reserves);
            info!("   complete: {}", bc.complete);

            (bc.real_token_reserves, bc.virtual_token_reserves, bc.virtual_sol_reserves)
        };

        // 1. 检查余额（包含 tip 费用）
        self.check_balance_for_operations(sol_amount, "买入操作")?;

        // 2. 构建交易指令（包含所有 tips）
        let instructions = self.build_buy_instructions_with_all_tips(
            mint,
            bonding_curve,
            associated_bonding_curve,
            sol_amount,
            real_token_reserves,      // 🔥 实际可买代币上限
            virtual_token_reserves,   // 🔥 用于价格公式计算
            virtual_sol_reserves,
        )?;

        info!("📦 交易指令已构建，共 {} 条指令", instructions.len());

        // 3. 构建 VersionedTransaction
        let transaction = self.build_versioned_transaction(instructions)?;

        // 4. 发送交易（SWQOS 优先，LightSpeed 保底）
        let signature = self.send_transaction_with_priority(transaction).await?;

        info!("✅ 买入交易已发送: {}", signature);

        // 5. 监控交易状态
        let confirmed = self.monitor_transaction_status(&signature, 30).await?;

        if confirmed {
            info!("🎉 买入交易已确认: {}", signature);
        } else {
            warn!("⚠️  买入交易未在规定时间内确认: {}", signature);
        }

        Ok(signature)
    }

    /// 检查余额是否足够执行操作
    ///
    /// 参考 lightspeed-examples/src/utils.ts:checkBalanceForOperations
    ///
    /// 🔥 修复: 计算所有 tips（LightSpeed + SWQOS）
    fn check_balance_for_operations(
        &self,
        required_lamports: u64,
        description: &str,
    ) -> Result<()> {
        let balance = self.rpc_client.get_balance(&self.payer.pubkey())
            .context("获取账户余额失败")?;

        // 🔥 修复: 计算所有 tip 费用
        let mut total_tips = 0u64;

        // 1. LightSpeed tip
        if self.config.use_lightspeed {
            total_tips += self.config.get_lightspeed_tip_lamports();
        }

        // 2. SWQOS tips（如果启用）
        let swqos_tips_total = if let Some(swqos) = &self.swqos_manager {
            match swqos.get_all_tip_instructions(&self.payer.pubkey()) {
                Ok(tips) => {
                    let mut swqos_total = 0u64;
                    for (service_name, tip_ix) in tips {
                        // 🔥 从 transfer 指令中提取 lamports（第3个参数）
                        if tip_ix.data.len() >= 12 {
                            let tip_amount = u64::from_le_bytes(
                                tip_ix.data[4..12].try_into().unwrap_or([0u8; 8])
                            );
                            swqos_total += tip_amount;
                            debug!("   SWQOS {} tip: {} lamports", service_name, tip_amount);
                        }
                    }
                    total_tips += swqos_total;
                    swqos_total
                }
                Err(e) => {
                    warn!("⚠️  获取 SWQOS tips 失败: {}", e);
                    0
                }
            }
        } else {
            0
        };

        // 计算总需求
        let total_required = required_lamports + total_tips;

        if balance < total_required {
            error!("❌ 余额不足 - {}", description);
            error!("   当前余额: {} SOL", balance as f64 / 1_000_000_000.0);
            error!("   需要金额: {} SOL", required_lamports as f64 / 1_000_000_000.0);
            if self.config.use_lightspeed {
                error!("   LightSpeed tip: {} SOL",
                    self.config.get_lightspeed_tip_lamports() as f64 / 1_000_000_000.0);
            }
            if swqos_tips_total > 0 {
                error!("   SWQOS tips: {} SOL", swqos_tips_total as f64 / 1_000_000_000.0);
            }
            error!("   总计需要: {} SOL", total_required as f64 / 1_000_000_000.0);
            return Err(anyhow::anyhow!("余额不足"));
        }

        info!("✅ 余额检查通过 - {}", description);
        info!("   当前余额: {} SOL", balance as f64 / 1_000_000_000.0);
        info!("   需要金额: {} SOL", required_lamports as f64 / 1_000_000_000.0);
        if self.config.use_lightspeed {
            info!("   LightSpeed tip: {} SOL",
                self.config.get_lightspeed_tip_lamports() as f64 / 1_000_000_000.0);
        }
        if swqos_tips_total > 0 {
            info!("   SWQOS tips: {} SOL", swqos_tips_total as f64 / 1_000_000_000.0);
        }
        info!("   总计需要: {} SOL", total_required as f64 / 1_000_000_000.0);
        info!("   剩余余额: {} SOL", (balance - total_required) as f64 / 1_000_000_000.0);

        Ok(())
    }

    // 🔥 已删除 build_buy_instructions（旧版非 tips 路径）
    // 生产环境统一使用 build_buy_instructions_with_all_tips（包含滑点保护、real_token_reserves、SWQOS tips）
    // 避免误用导致上链失败

    /// 获取 Associated Token Address
    /// 使用 PDA 派生，避免依赖外部库
    /// 🔥 修复: 支持 Token-2022
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

    /// 🔥 新增: 获取支持 Token-2022 的 ATA 地址
    fn get_ata_with_program(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
        let associated_token_program_id = Pubkey::try_from("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
            .expect("Invalid ASSOCIATED_TOKEN_PROGRAM_ID");

        Pubkey::find_program_address(
            &[
                wallet.as_ref(),
                token_program.as_ref(),  // 🔥 使用实际的 token program
                mint.as_ref(),
            ],
            &associated_token_program_id,
        )
        .0
    }

    /// 派生 creator_vault PDA（完全参考 sol-trade-sdk）
    /// 🔥 修复: creator_vault 是 PDA，不是 ATA！
    /// seed = [b"creator-vault", creator.as_ref()]
    /// program_id = PUMPFUN_PROGRAM_ID
    fn derive_creator_vault(creator: &Pubkey) -> Result<Pubkey> {
        let pumpfun_program = Pubkey::try_from(PUMPFUN_PROGRAM_ID)?;

        let (creator_vault, _bump) = Pubkey::find_program_address(
            &[
                b"creator-vault",
                creator.as_ref(),
            ],
            &pumpfun_program,
        );

        Ok(creator_vault)
    }

    /// 🔥 新增: 从 bonding_curve 账户读取 creator
    fn get_creator_from_bonding_curve(&self, bonding_curve: &Pubkey) -> Result<Pubkey> {
        use crate::grpc::parser::bonding_curve_decode;

        let data = self.rpc_client.get_account_data(bonding_curve)
            .context("读取 bonding curve 账户失败")?;

        let bc = bonding_curve_decode(&data)
            .ok_or_else(|| anyhow::anyhow!("解码 bonding curve 失败"))?;

        Ok(bc.creator)
    }

    /// 派生 user_volume_accumulator PDA（完全参考 sol-trade-sdk）
    /// 🔥 修复: seed 必须是 "user_volume_accumulator" (underscore)，不是 hyphen!
    fn derive_user_volume_accumulator(user: &Pubkey) -> Result<Pubkey> {
        let pumpfun_program = Pubkey::try_from(PUMPFUN_PROGRAM_ID)?;

        let (user_volume_accumulator, _bump) = Pubkey::find_program_address(
            &[
                b"user_volume_accumulator",  // 🔥 修复: underscore，不是 hyphen
                user.as_ref(),
            ],
            &pumpfun_program,
        );

        Ok(user_volume_accumulator)
    }

    /// 🔥 修复: 计算买入应得的代币数量（完全参考 sol-trade-sdk）
    ///
    /// 参考: sol-trade-sdk/src/utils/calc/pumpfun.rs:get_buy_token_amount_from_sol_amount
    /// 🔥 修复: 使用 virtual_token_reserves 计算，再 min(real_token_reserves)
    /// 🔥 修复: 使用正确的费率 FEE_BASIS_POINTS=95 + CREATOR_FEE=30
    fn calculate_buy_token_amount(
        real_token_reserves: u64,      // 实际可买代币上限
        virtual_token_reserves: u64,   // 用于价格公式计算
        virtual_sol_reserves: u64,
        sol_amount: u64,
    ) -> u64 {
        if sol_amount == 0 {
            return 0;
        }

        if virtual_token_reserves == 0 || virtual_sol_reserves == 0 {
            return 0;
        }

        // 🔥 修复: PumpFun 费率（完全对齐 sol-trade-sdk）
        // FEE_BASIS_POINTS = 95 (0.95%)
        // CREATOR_FEE = 30 (0.30%)
        // 总费率 = 125 bps (1.25%)
        const FEE_BASIS_POINTS: u128 = 95;
        const CREATOR_FEE: u128 = 30;
        const BASIS_POINTS: u128 = 10_000;
        let total_fee_basis_points = FEE_BASIS_POINTS + CREATOR_FEE;

        // 扣除手续费后的输入金额（使用 checked 操作）
        let amount_128 = sol_amount as u128;
        let input_amount = amount_128
            .checked_mul(BASIS_POINTS)
            .unwrap_or(0)
            .checked_div(total_fee_basis_points + BASIS_POINTS)
            .unwrap_or(0);

        if input_amount == 0 {
            return 0;
        }

        // 恒定乘积公式: k = x * y（使用 checked 操作）
        // 🔥 修复: 使用 virtual_token_reserves 计算（对齐 SDK）
        let denominator = (virtual_sol_reserves as u128) + input_amount;
        let tokens_received = input_amount
            .checked_mul(virtual_token_reserves as u128)  // 🔥 使用 virtual
            .unwrap_or(0)
            .checked_div(denominator)
            .unwrap_or(0);

        // 🔥 修复: 取 min(计算值, real_token_reserves) 确保不超过实际可买
        let tokens_u64 = tokens_received.min(u64::MAX as u128) as u64;
        tokens_u64.min(real_token_reserves)
    }

    /// 🔥 新增: 计算带滑点保护的最大 SOL 成本
    ///
    /// 参考: sol-trade-sdk/src/utils/calc/common.rs:calculate_with_slippage_buy
    fn calculate_max_sol_cost_with_slippage(
        sol_amount: u64,
        slippage_percent: f64,
    ) -> u64 {
        let slippage_basis_points = (slippage_percent * 100.0) as u64; // 3% -> 300 bps
        let amount_128 = sol_amount as u128;
        let slippage_128 = slippage_basis_points as u128;

        // 使用 saturating 操作防止溢出
        let max_cost = amount_128
            .saturating_mul(10_000 + slippage_128)
            .checked_div(10_000)
            .unwrap_or(sol_amount as u128);

        max_cost as u64
    }

    /// 发送交易（带重试机制）
    ///
    /// 参考 lightspeed-examples/src/utils.ts:sendTxWithRetries
    ///
    /// 配置:
    /// - preflightCommitment: "processed"
    /// - skipPreflight: true
    /// - maxRetries: 3
    async fn send_tx_with_retries(
        &self,
        instructions: Vec<Instruction>,
        max_attempts: u32,
    ) -> Result<Signature> {
        info!("📤 准备发送交易，最多重试 {} 次", max_attempts);

        // 获取最新 blockhash
        let recent_blockhash = self.rpc_client.get_latest_blockhash()
            .context("获取 blockhash 失败")?;

        // 构建交易
        let mut transaction = Transaction::new_with_payer(
            &instructions,
            Some(&self.payer.pubkey()),
        );
        transaction.sign(&[&*self.payer], recent_blockhash);

        // 序列化交易
        let serialized_tx = bincode::serialize(&transaction)
            .context("序列化交易失败")?;

        debug!("📦 交易大小: {} bytes", serialized_tx.len());

        // 选择 RPC 客户端（优先使用 LightSpeed，否则使用普通 RPC）
        let rpc_to_use = if let Some(ref lightspeed) = self.lightspeed_rpc {
            debug!("🚀 使用 LightSpeed RPC 发送交易");
            lightspeed
        } else {
            debug!("📡 使用普通 RPC 发送交易");
            &self.rpc_client
        };

        // 重试发送
        for attempt in 1..=max_attempts {
            info!("🔄 发送尝试 {}/{}", attempt, max_attempts);

            match rpc_to_use.send_transaction_with_config(
                &transaction,
                solana_client::rpc_config::RpcSendTransactionConfig {
                    skip_preflight: true,
                    preflight_commitment: Some(solana_commitment_config::CommitmentLevel::Processed),
                    max_retries: Some(3),
                    ..Default::default()
                },
            ) {
                Ok(signature) => {
                    info!("✅ 交易已发送 (尝试 {}): {}", attempt, signature);
                    return Ok(signature);
                }
                Err(e) => {
                    error!("❌ 发送失败 (尝试 {}): {:?}", attempt, e);
                    if attempt == max_attempts {
                        return Err(anyhow::anyhow!("达到最大重试次数: {:?}", e));
                    }
                    // 等待一小段时间再重试
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }

        Err(anyhow::anyhow!("发送交易失败"))
    }

    /// 监控交易状态
    ///
    /// 参考 lightspeed-examples/src/utils.ts:monitorTransactionStatus
    ///
    /// 持续检查交易状态，直到确认或超时
    async fn monitor_transaction_status(
        &self,
        signature: &Signature,
        max_wait_seconds: u64,
    ) -> Result<bool> {
        info!("⏳ 开始监控交易状态: {}", signature);
        info!("   最大等待时间: {} 秒", max_wait_seconds);

        let start_time = Instant::now();
        let max_wait = Duration::from_secs(max_wait_seconds);

        while start_time.elapsed() < max_wait {
            match self.rpc_client.get_signature_status(signature) {
                Ok(Some(status)) => {
                    match status {
                        Ok(_) => {
                            // 交易成功
                            let elapsed = start_time.elapsed().as_secs();
                            info!("✅ 交易已确认 (耗时 {} 秒)", elapsed);
                            return Ok(true);
                        }
                        Err(e) => {
                            // 交易失败
                            error!("❌ 交易失败: {:?}", e);
                            return Ok(false);
                        }
                    }
                }
                Ok(None) => {
                    // 交易尚未确认，继续等待
                    debug!("⏳ 交易尚未确认，继续等待...");
                }
                Err(e) => {
                    warn!("⚠️  查询交易状态失败: {:?}", e);
                }
            }

            // 等待 1 秒后再次检查
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // 超时
        warn!("⏰ 交易确认超时 ({} 秒)", max_wait_seconds);
        Ok(false)
    }

    /// 获取账户余额
    pub fn get_balance(&self) -> Result<u64> {
        self.rpc_client.get_balance(&self.payer.pubkey())
            .context("获取账户余额失败")
    }

    /// 构建买入指令（包含所有 tips：LightSpeed + SWQOS）
    ///
    /// 🔥 修复: 使用 virtual_token_reserves 计算，再 min(real_token_reserves)
    fn build_buy_instructions_with_all_tips(
        &self,
        mint: &Pubkey,
        bonding_curve: &Pubkey,
        associated_bonding_curve: &Pubkey,
        sol_amount: u64,
        real_token_reserves: u64,      // 🔥 实际可买代币上限
        virtual_token_reserves: u64,   // 🔥 用于价格公式计算
        virtual_sol_reserves: u64,
    ) -> Result<Vec<Instruction>> {
        let mut instructions = Vec::new();
        let payer = self.payer.pubkey();

        // 🔥 修复: 移除重复的 ComputeBudget 指令（保留最后的 insert 版本）

        // 🔥 新增: 检测 Token Program（支持 Token-2022）
        let token_program = self.detect_token_program(mint)?;

        // 1. 创建用户的 Token ATA（如果不存在）
        // 🔥 修复: 使用检测到的 token program（支持 Token-2022）
        let user_token_account = Self::get_ata_with_program(&payer, mint, &token_program);

        debug!("🏗️  添加 ATA 创建指令");
        debug!("   Token Program: {}", token_program);
        debug!("   用户 Token 账户: {}", user_token_account);

        // 手动构建 CreateIdempotent 指令（幂等）
        let ata_program_id = Pubkey::try_from("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")?;
        let system_program_id = Pubkey::try_from(SYSTEM_PROGRAM)?;

        let create_ata_ix = Instruction {
            program_id: ata_program_id,
            accounts: vec![
                AccountMeta::new(payer, true),                    // 0. 支付者（signer）
                AccountMeta::new(user_token_account, false),      // 1. 关联代币账户
                AccountMeta::new_readonly(payer, false),          // 2. 拥有者
                AccountMeta::new_readonly(*mint, false),          // 3. mint
                AccountMeta::new_readonly(system_program_id, false), // 4. system_program
                AccountMeta::new_readonly(token_program, false),  // 5. token_program (动态)
            ],
            data: vec![1], // 1 = CreateIdempotent 指令
        };
        instructions.push(create_ata_ix);

        // 2. 构建 PumpFun 买入指令（完全参考 sol-trade-sdk 的账户顺序）
        debug!("🏗️  构建 PumpFun 买入指令");

        // 🔥 修复: 先读取 creator，再派生 creator_vault PDA
        let creator = self.get_creator_from_bonding_curve(bonding_curve)?;
        let creator_vault = Self::derive_creator_vault(&creator)?;
        debug!("   Creator: {}", creator);
        debug!("   Creator Vault: {}", creator_vault);

        // 派生 user_volume_accumulator PDA
        let user_volume_accumulator = Self::derive_user_volume_accumulator(&payer)?;
        debug!("   User Volume Accumulator: {}", user_volume_accumulator);

        // 🔥 修复: 正确计算 token_amount 和 max_sol_cost（参考 sol-trade-sdk）
        // 使用 virtual_token_reserves 计算，再 min(real_token_reserves)
        let token_amount = Self::calculate_buy_token_amount(
            real_token_reserves,      // 🔥 实际可买代币上限
            virtual_token_reserves,   // 🔥 用于价格公式计算
            virtual_sol_reserves,
            sol_amount,
        );
        let max_sol_cost = Self::calculate_max_sol_cost_with_slippage(
            sol_amount,
            self.config.slippage_percent,
        );

        info!("📊 买入计算:");
        info!("   输入 SOL: {} ({} lamports)", sol_amount as f64 / 1e9, sol_amount);
        info!("   期望代币数量: {} tokens", token_amount);
        info!("   最大 SOL 成本 (含{}%滑点): {} lamports", self.config.slippage_percent, max_sol_cost);

        // 构建指令数据
        // 格式: [discriminator(8), token_amount(8), max_sol_cost(8)]
        let mut instruction_data = Vec::with_capacity(24);
        instruction_data.extend_from_slice(&BUY_DISCRIMINATOR);
        instruction_data.extend_from_slice(&token_amount.to_le_bytes());    // 🔥 修复: token_amount
        instruction_data.extend_from_slice(&max_sol_cost.to_le_bytes());    // 🔥 修复: max_sol_cost

        // 构建账户列表（完全参考 sol-trade-sdk 的顺序，16 个账户）
        let accounts = vec![
            AccountMeta::new_readonly(self.global, false),                          // 0: global
            AccountMeta::new(self.fee_recipient, false),                            // 1: fee_recipient
            AccountMeta::new_readonly(*mint, false),                                // 2: mint
            AccountMeta::new(*bonding_curve, false),                                // 3: bonding_curve
            AccountMeta::new(*associated_bonding_curve, false),                     // 4: associated_bonding_curve
            AccountMeta::new(user_token_account, false),                            // 5: user_token_account
            AccountMeta::new(payer, true),                                          // 6: payer (signer)
            AccountMeta::new_readonly(Pubkey::try_from(SYSTEM_PROGRAM).unwrap(), false), // 7: system_program
            AccountMeta::new_readonly(Pubkey::try_from(SYSTEM_TOKEN_PROGRAM).unwrap(), false), // 8: token_program (固定 Token v3，对齐 SDK) ⭐
            AccountMeta::new(creator_vault, false),                                 // 9: creator_vault ⭐
            AccountMeta::new_readonly(self.event_authority, false),                 // 10: event_authority
            AccountMeta::new_readonly(self.pumpfun_program, false),                 // 11: pumpfun_program
            AccountMeta::new(Pubkey::try_from(GLOBAL_VOLUME_ACCUMULATOR).unwrap(), false), // 12: global_volume_accumulator ⭐ (可写)
            AccountMeta::new(user_volume_accumulator, false),                       // 13: user_volume_accumulator ⭐
            AccountMeta::new_readonly(Pubkey::try_from(FEE_CONFIG).unwrap(), false), // 14: fee_config ⭐
            AccountMeta::new_readonly(Pubkey::try_from(FEE_PROGRAM).unwrap(), false), // 15: fee_program ⭐
        ];

        // 🔥 排障日志: 打印关键账户表摘要
        debug!("📋 PumpFun 买入账户表摘要 (16 accounts):");
        debug!("   [0] global: {} (readonly)", self.global);
        debug!("   [1] fee_recipient: {} (writable)", self.fee_recipient);
        debug!("   [8] token_program: {} (readonly, Token v3 固定) ⭐",
            Pubkey::try_from(SYSTEM_TOKEN_PROGRAM).unwrap()
        );
        debug!("   [9] creator_vault: {} (writable) ⭐", creator_vault);
        debug!("   [12] global_volume_accumulator: {} (writable) ⭐",
            Pubkey::try_from(GLOBAL_VOLUME_ACCUMULATOR).unwrap()
        );
        debug!("   [13] user_volume_accumulator: {} (writable) ⭐", user_volume_accumulator);
        debug!("   [14] fee_config: {} (readonly) ⭐", Pubkey::try_from(FEE_CONFIG).unwrap());
        debug!("   [15] fee_program: {} (readonly) ⭐", Pubkey::try_from(FEE_PROGRAM).unwrap());

        instructions.push(Instruction {
            program_id: self.pumpfun_program,
            accounts,
            data: instruction_data,
        });

        // 3. 添加 LightSpeed tip（如果启用）
        if self.config.use_lightspeed {
            let tip_address = self.config.lightspeed_tip_address.parse::<Pubkey>()
                .context("Invalid lightspeed_tip_address")?;
            let tip_lamports = self.config.get_lightspeed_tip_lamports();

            info!("💨 添加 LightSpeed tip: {} SOL", tip_lamports as f64 / 1_000_000_000.0);

            instructions.push(transfer(&payer, &tip_address, tip_lamports));
        }

        // 4. 添加 SWQOS tips（如果启用）
        if let Some(swqos) = &self.swqos_manager {
            match swqos.get_all_tip_instructions(&payer) {
                Ok(swqos_tips) => {
                    let tips_count = swqos_tips.len();
                    for (service_name, tip_ix) in swqos_tips {
                        instructions.push(tip_ix);
                        debug!("💰 添加 {} tip 指令", service_name);
                    }
                    info!("✅ 已添加 {} 个 SWQOS tip 指令", tips_count);
                }
                Err(e) => {
                    warn!("⚠️  获取 SWQOS tip 指令失败: {}", e);
                }
            }
        }

        // 1. 添加计算预算指令（最后插入到开头，完全参考 lightspeed-examples 的 unshift 逻辑）
        debug!("📊 添加 ComputeBudget 指令");
        instructions.insert(0, ComputeBudgetInstruction::set_compute_unit_price(
            self.config.compute_unit_price,
        ));
        instructions.insert(0, ComputeBudgetInstruction::set_compute_unit_limit(
            self.config.compute_unit_limit,
        ));

        Ok(instructions)
    }

    /// 构建 VersionedTransaction
    fn build_versioned_transaction(&self, instructions: Vec<Instruction>) -> Result<VersionedTransaction> {
        let recent_blockhash = self.rpc_client.get_latest_blockhash()
            .context("获取 blockhash 失败")?;

        let message = v0::Message::try_compile(
            &self.payer.pubkey(),
            &instructions,
            &[],  // address_lookup_tables
            recent_blockhash,
        ).context("编译消息失败")?;

        let versioned_message = VersionedMessage::V0(message);

        let transaction = VersionedTransaction::try_new(
            versioned_message,
            &[&*self.payer]
        ).context("创建交易失败")?;

        Ok(transaction)
    }

    /// 发送交易（优先级：SWQOS > LightSpeed）
    async fn send_transaction_with_priority(&self, transaction: VersionedTransaction) -> Result<Signature> {
        // 优先使用 SWQOS 田忌赛马
        if let Some(swqos) = &self.swqos_manager {
            info!("🏁 尝试使用 SWQOS 田忌赛马发送...");

            match swqos.send_transaction_race(&transaction).await {
                Ok(result) => {
                    info!("✅ SWQOS 成功: {} ({}ms)", result.service_name, result.latency_ms);
                    return result.signature.ok_or_else(|| anyhow::anyhow!("SWQOS 成功但无签名"));
                }
                Err(e) => {
                    warn!("⚠️  SWQOS 所有重试都失败: {}", e);
                    warn!("   尝试使用 LightSpeed 保底...");
                }
            }
        }

        // SWQOS 失败或未启用，使用 LightSpeed
        info!("📡 使用 LightSpeed RPC 发送...");
        self.send_via_lightspeed(&transaction).await
    }

    /// 通过 LightSpeed RPC 发送交易
    async fn send_via_lightspeed(&self, transaction: &VersionedTransaction) -> Result<Signature> {
        let signature = transaction.signatures[0];

        // 选择 RPC 客户端（优先使用 LightSpeed，否则使用普通 RPC）
        let rpc_to_use = if let Some(ref lightspeed) = self.lightspeed_rpc {
            debug!("🚀 使用 LightSpeed RPC 发送交易");
            lightspeed
        } else {
            debug!("📡 使用普通 RPC 发送交易");
            &self.rpc_client
        };

        // 重试发送
        let max_attempts = 3;
        for attempt in 1..=max_attempts {
            debug!("🔄 发送尝试 {}/{}", attempt, max_attempts);

            match rpc_to_use.send_transaction_with_config(
                transaction,
                solana_client::rpc_config::RpcSendTransactionConfig {
                    skip_preflight: true,
                    preflight_commitment: Some(solana_commitment_config::CommitmentLevel::Processed),
                    max_retries: Some(3),
                    ..Default::default()
                },
            ) {
                Ok(sig) => {
                    info!("✅ 发送成功 (尝试 {}): {}", attempt, sig);
                    return Ok(sig);
                }
                Err(e) => {
                    if attempt < max_attempts {
                        warn!("⚠️  发送失败 (尝试 {}): {}", attempt, e);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    } else {
                        error!("❌ 所有尝试都失败: {}", e);
                        return Err(e.into());
                    }
                }
            }
        }

        Ok(signature)
    }
}

