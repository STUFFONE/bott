use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use std::time::Duration;
use tonic::transport::channel::ClientTlsConfig;
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::geyser::{
    subscribe_update::UpdateOneof, SubscribeRequest,
    SubscribeRequestFilterAccounts, SubscribeRequestFilterTransactions, SubscribeUpdate,
};
use yellowstone_grpc_proto::prelude::CommitmentLevel;
use solana_sdk::pubkey::Pubkey;
use crossbeam_queue::ArrayQueue;  // 🔥 新增: 无锁队列
use std::sync::Arc;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};  // 🔥 新增: base64解码

use crate::types::SniperEvent;

use super::parser::parse_pumpfun_event;

const PUMPFUN_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// Yellowstone gRPC 客户端
#[derive(Clone)]
pub struct GrpcClient {
    endpoint: String,
    x_token: Option<String>,
}

impl GrpcClient {
    /// 创建新的 gRPC 客户端
    pub fn new(endpoint: String, x_token: Option<String>) -> Self {
        Self {
            endpoint,
            x_token,
        }
    }

    /// 订阅 PumpFun 事件（带自动重连）
    ///
    /// 无限循环重试，断线后立即重连
    /// 🔥 修复: 使用指数退避重连延迟，避免疯狂重连
    /// 🔥 优化: 使用无锁队列 ArrayQueue 替代 mpsc channel
    pub async fn subscribe_with_reconnect(&self, event_queue: Arc<ArrayQueue<SniperEvent>>) {
        let mut retry_count = 0u32;

        loop {
            info!("🔌 尝试连接 gRPC 服务器 (尝试 #{})", retry_count + 1);

            match self.subscribe_pumpfun_events(event_queue.clone()).await {
                Ok(_) => {
                    warn!("⚠️  gRPC 订阅正常结束（不应该发生），准备重连...");
                    retry_count = 0; // 重置重试计数
                }
                Err(e) => {
                    error!("❌ gRPC 连接失败: {}", e);
                    retry_count += 1;
                }
            }

            // 🔥 修复: 指数退避重连延迟（5ms -> 10ms -> 20ms -> ... -> 最多5秒）
            let delay_ms = std::cmp::min(5 * (1 << retry_count.min(10)), 5000);
            info!("⏳ {}ms 后重连...", delay_ms);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    /// 订阅 PumpFun 事件（单次，不重连）
    /// 🔥 优化: 使用无锁队列 ArrayQueue
    pub async fn subscribe_pumpfun_events(
        &self,
        event_queue: Arc<ArrayQueue<SniperEvent>>,
    ) -> Result<()> {
        info!("🔌 连接到 gRPC 服务器: {}", self.endpoint);

        // 使用 yellowstone-grpc-client 创建连接（支持 x_token）
        let mut client = GeyserGrpcClient::build_from_shared(self.endpoint.clone())
            .context("Invalid gRPC endpoint")?
            .x_token(self.x_token.clone())
            .context("Failed to set x_token")?
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .context("Failed to set TLS config")?
            .max_decoding_message_size(64 * 1024 * 1024) // 64 MB
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .connect()
            .await
            .context("Failed to connect to gRPC server")?;

        info!("✅ 成功连接到 gRPC 服务器");
        if self.x_token.is_some() {
            info!("🔐 使用 X-Token 认证");
        }

        // 构建订阅请求
        let mut accounts_filter = std::collections::HashMap::new();
        accounts_filter.insert(
            "pumpfun".to_string(),
            SubscribeRequestFilterAccounts {
                account: vec![],
                owner: vec![PUMPFUN_PROGRAM_ID.to_string()],
                filters: vec![],
                nonempty_txn_signature: None,
            },
        );

        let mut transactions_filter = std::collections::HashMap::new();
        transactions_filter.insert(
            "pumpfun".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                signature: None,
                account_include: vec![PUMPFUN_PROGRAM_ID.to_string()],
                account_exclude: vec![],
                account_required: vec![],
            },
        );

        let request = SubscribeRequest {
            accounts: accounts_filter,
            transactions: transactions_filter,
            slots: std::collections::HashMap::new(),
            blocks: std::collections::HashMap::new(),
            blocks_meta: std::collections::HashMap::new(),
            entry: std::collections::HashMap::new(),
            commitment: Some(CommitmentLevel::Confirmed as i32),
            accounts_data_slice: vec![],
            ping: None,
            transactions_status: std::collections::HashMap::new(),
            from_slot: None,
        };

        info!("📡 订阅 PumpFun 事件...");

        // 发起订阅（yellowstone-grpc-client 返回 (Sender, Receiver)）
        let (mut subscribe_tx, mut stream) = client
            .subscribe()
            .await
            .context("Failed to subscribe")?;

        // 发送订阅请求
        subscribe_tx
            .send(request)
            .await
            .context("Failed to send subscribe request")?;

        info!("✅ 成功订阅 PumpFun 事件");

        // 处理事件流（阻塞等待直到流结束或错误）
        while let Some(result) = stream.next().await {
            match result {
                Ok(update) => {
                    if let Err(e) = Self::handle_update(update, &event_queue).await {
                        error!("Error handling update: {}", e);
                    }
                }
                Err(e) => {
                    error!("❌ gRPC 流错误: {}", e);
                    return Err(anyhow::anyhow!("gRPC stream error: {}", e));
                }
            }
        }

        warn!("⚠️  gRPC 事件流结束");
        Err(anyhow::anyhow!("Event stream ended unexpectedly"))
    }

    /// 处理订阅更新
    /// 🔥 优化: 使用无锁队列 ArrayQueue
    async fn handle_update(
        update: SubscribeUpdate,
        event_queue: &Arc<ArrayQueue<SniperEvent>>,
    ) -> Result<()> {
        match update.update_oneof {
            Some(UpdateOneof::Transaction(tx_update)) => {
                // 解析交易中的 PumpFun 事件
                if let Some(transaction) = tx_update.transaction {
                    let signature = bs58::encode(&transaction.signature).into_string();

                    // 解析交易中的指令和日志
                    if let Some(meta) = transaction.meta {
                        // 🔥 修复: 从 transaction.transaction 中提取账户和指令
                        let (account_keys, instructions) = if let Some(ref tx) = transaction.transaction {
                            // tx 是 solana_transaction_status::proto::Transaction
                            let account_keys: Vec<Pubkey> = tx.message.as_ref()
                                .map(|msg| {
                                    msg.account_keys.iter()
                                        .filter_map(|k| {
                                            if k.len() == 32 {
                                                let mut arr = [0u8; 32];
                                                arr.copy_from_slice(k);
                                                Some(Pubkey::new_from_array(arr))
                                            } else {
                                                None
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            let instructions = tx.message.as_ref()
                                .map(|msg| msg.instructions.clone())
                                .unwrap_or_default();

                            (account_keys, instructions)
                        } else {
                            (Vec::new(), Vec::new())
                        };

                        // 🔥 检测是否为 token 创建交易（参考 sol-parser-sdk）
                        // CreateToken 事件的 discriminator（前 16 字节）
                        // base64 "GB7IKAUcB3c..." 解码后的 discriminator
                        let is_created_buy = meta.log_messages.iter()
                            .any(|log| {
                                if let Some(data_start) = log.find("Program data: ") {
                                    let data_str = &log[data_start + 14..];
                                    if let Ok(decoded) = BASE64_STANDARD.decode(data_str.trim()) {
                                        // CreateToken 事件 discriminator（前 8-16 字节）
                                        // 参考 sol-parser-sdk 的匹配逻辑
                                        return decoded.len() >= 16 && decoded.starts_with(&[0x18, 0x1e, 0xc8, 0x28, 0x05, 0x1c, 0x07, 0x77]);
                                    }
                                }
                                false
                            });

                        // 从日志中解析事件
                        for log in &meta.log_messages {
                            // 🔥 修复：PumpFun 事件日志格式是 "Program data: <base64>"
                            // 不是 "Program log: Instruction:"
                            if log.contains("Program data:") {
                                // 尝试解析 PumpFun 事件
                                if let Ok(Some(mut event)) =
                                    parse_pumpfun_event(log, &signature, tx_update.slot, is_created_buy)
                                {
                                    // 🔥 补全账户信息
                                    Self::enrich_event_with_accounts(&mut event, &account_keys, &instructions);

                                    debug!("Parsed PumpFun event: {:?}", event);
                                    // 🔥 优化: 使用无锁队列推送事件
                                    if event_queue.push(event).is_err() {
                                        error!("❌ 事件队列已满，丢弃事件");
                                    }
                                }
                            }
                        }

                        // 🔥 新增: 从内部指令中补全账户信息
                        // 某些情况下，PumpFun 指令可能作为 CPI 调用出现在 inner_instructions 中
                        //
                        // 📝 设计说明：当前无需在此补全账户，因为：
                        //    1. 事件从 log_messages 解析，不是从 instructions 解析
                        //    2. 账户已在外层补全（line 237），使用整个交易的 account_keys
                        //    3. account_keys 包含所有账户（含 inner_instructions 涉及的）
                        //    4. 此分支仅用于检测 CPI 场景（调试用途）
                        //
                        // ⚠️ 如实网遇到 CPI 场景账户缺失，可在此调用 enrich_event_with_accounts
                        for inner_instruction in &meta.inner_instructions {
                            for instruction in &inner_instruction.instructions {
                                // 检查是否是 PumpFun 程序指令
                                if (instruction.program_id_index as usize) < account_keys.len() {
                                    let program_id = account_keys[instruction.program_id_index as usize];

                                    if program_id.to_string() == PUMPFUN_PROGRAM_ID {
                                        debug!("🔍 发现 inner_instruction 中的 PumpFun 指令");

                                        // 如果之前已经解析出事件但账户不完整，可以再次尝试补全
                                        // 这里的逻辑是：inner_instructions 可能包含额外的账户信息
                                        // 参考: solana-streamer 的实现方式
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Some(UpdateOneof::Ping(_)) => {
                debug!("Received ping");
            }
            Some(UpdateOneof::Pong(_)) => {
                debug!("Received pong");
            }
            _ => {}
        }

        Ok(())
    }

    /// 🔥 修复: 从交易指令中提取账户信息并补全事件数据
    fn enrich_event_with_accounts(
        event: &mut SniperEvent,
        account_keys: &[Pubkey],
        instructions: &[yellowstone_grpc_proto::solana::storage::confirmed_block::CompiledInstruction],
    ) {
        use super::parser::extract_pumpfun_accounts;

        for instruction in instructions {
            // 检查是否是 PumpFun 程序指令
            if (instruction.program_id_index as usize) < account_keys.len() {
                let program_id = account_keys[instruction.program_id_index as usize];

                if program_id.to_string() == PUMPFUN_PROGRAM_ID {
                    // 🔥 修复: 将 u8 账户索引转换为 u32
                    let account_indices: Vec<u32> = instruction.accounts.iter()
                        .map(|&idx| idx as u32)
                        .collect();

                    // 尝试解析指令获取账户
                    if let Some(accounts) = extract_pumpfun_accounts(
                        account_keys,
                        &instruction.data,
                        &account_indices,
                    ) {
                        // 补全事件数据并验证 mint 一致性
                        match event {
                            SniperEvent::Trade(ref mut trade) => {
                                // 验证 mint 一致性
                                if trade.mint != accounts.mint {
                                    warn!("⚠️  Trade 事件 mint 不一致: event={}, instruction={}",
                                        trade.mint, accounts.mint);
                                }
                                // 补全所有账户信息
                                trade.bonding_curve = accounts.bonding_curve;
                                trade.associated_bonding_curve = accounts.associated_bonding_curve;
                                trade.creator_vault = accounts.creator_vault;
                                trade.associated_user = accounts.associated_user;
                                trade.global_volume_accumulator = accounts.global_volume_accumulator;
                                trade.user_volume_accumulator = accounts.user_volume_accumulator;
                                debug!("✅ 补全 Trade 事件账户: mint={}, bonding_curve={}, associated_bonding_curve={}, creator_vault={}, associated_user={}, global_volume_accumulator={}, user_volume_accumulator={}",
                                    accounts.mint, accounts.bonding_curve, accounts.associated_bonding_curve, accounts.creator_vault,
                                    accounts.associated_user, accounts.global_volume_accumulator, accounts.user_volume_accumulator);
                            }
                            SniperEvent::CreateToken(ref mut create) => {
                                // 验证 mint 一致性
                                if create.mint != accounts.mint {
                                    warn!("⚠️  CreateToken 事件 mint 不一致: event={}, instruction={}",
                                        create.mint, accounts.mint);
                                }
                                create.associated_bonding_curve = accounts.associated_bonding_curve;
                                debug!("✅ 补全 CreateToken 事件账户: mint={}, associated_bonding_curve={}",
                                    accounts.mint, accounts.associated_bonding_curve);
                            }
                            SniperEvent::Migrate(ref mut migrate) => {
                                // 验证 mint 一致性
                                if migrate.mint != accounts.mint {
                                    warn!("⚠️  Migrate 事件 mint 不一致: event={}, instruction={}",
                                        migrate.mint, accounts.mint);
                                }
                                migrate.global = accounts.global;
                                migrate.withdraw_authority = accounts.withdraw_authority;
                                migrate.associated_bonding_curve = accounts.associated_bonding_curve;
                                debug!("✅ 补全 Migrate 事件账户: mint={}", accounts.mint);
                            }
                        }
                        break;  // 找到匹配的指令后退出
                    }
                }
            }
        }
    }
}

