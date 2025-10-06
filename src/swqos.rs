//! SWQOS (Solana Web Quality of Service) 多服务接入模块
//!
//! 完全参考 sol-trade-sdk 的 SWQOS 实现，支持多服务商并行发送
//! 实现田忌赛马策略：谁最快谁上链成功谁收小费，后面的全失败

use anyhow::Result;
use log::{info, warn, error, debug};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    signature::Signature,
    transaction::VersionedTransaction,
};
use std::{
    collections::HashMap,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::RwLock,
    time::timeout,
};
use reqwest::Client;
use base64::{Engine, engine::general_purpose::STANDARD};
// 🔥 注意: rand 0.9+ 使用 IndexedRandom trait，而非旧版的 SliceRandom
// SliceRandom 在 rand 0.9 中已移除 .choose() 方法，必须使用 IndexedRandom
use rand::prelude::IndexedRandom;

/// SWQOS 服务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SwqosType {
    Jito,
    NextBlock,
    ZeroSlot,
    Temporal,
    Bloxroute,
    Node1,
    FlashBlock,
    BlockRazor,
    Astralane,
    Default,
}

impl FromStr for SwqosType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "jito" => Ok(SwqosType::Jito),
            "nextblock" => Ok(SwqosType::NextBlock),
            "zeroslot" => Ok(SwqosType::ZeroSlot),
            "temporal" => Ok(SwqosType::Temporal),
            "bloxroute" => Ok(SwqosType::Bloxroute),
            "node1" => Ok(SwqosType::Node1),
            "flashblock" => Ok(SwqosType::FlashBlock),
            "blockrazor" => Ok(SwqosType::BlockRazor),
            "astralane" => Ok(SwqosType::Astralane),
            "default" => Ok(SwqosType::Default),
            _ => Err(anyhow::anyhow!("Unknown SWQOS type: {}", s)),
        }
    }
}

/// SWQOS 地区
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SwqosRegion {
    NewYork,
    Frankfurt,
    Amsterdam,
    SLC,
    Tokyo,
    London,
    LosAngeles,
    Default,
}

impl FromStr for SwqosRegion {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "newyork" | "ny" => Ok(SwqosRegion::NewYork),
            "frankfurt" | "fra" => Ok(SwqosRegion::Frankfurt),
            "amsterdam" | "ams" => Ok(SwqosRegion::Amsterdam),
            "slc" => Ok(SwqosRegion::SLC),
            "tokyo" | "tyo" => Ok(SwqosRegion::Tokyo),
            "london" | "lon" => Ok(SwqosRegion::London),
            "losangeles" | "la" | "lax" => Ok(SwqosRegion::LosAngeles),
            "default" => Ok(SwqosRegion::Default),
            _ => Err(anyhow::anyhow!("Unknown SWQOS region: {}", s)),
        }
    }
}

/// Tip账户常量 (从sol-trade-sdk复制)
const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

const NEXTBLOCK_TIP_ACCOUNTS: &[&str] = &[
    "NextbLoCkVtMGcV47JzewQdvBpLqT9TxQFozQkN98pE",
    "NexTbLoCkWykbLuB1NkjXgFWkX9oAtcoagQegygXXA2",
    "NeXTBLoCKs9F1y5PJS9CKrFNNLU1keHW71rfh7KgA1X",
    "NexTBLockJYZ7QD7p2byrUa6df8ndV2WSd8GkbWqfbb",
    "neXtBLock1LeC67jYd1QdAa32kbVeubsfPNTJC1V5At",
    "nEXTBLockYgngeRmRrjDV31mGSekVPqZoMGhQEZtPVG",
    "NEXTbLoCkB51HpLBLojQfpyVAMorm3zzKg7w9NFdqid",
    "nextBLoCkPMgmG8ZgJtABeScP35qLa2AMCNKntAP7Xc",
];

const ZEROSLOT_TIP_ACCOUNTS: &[&str] = &[
    "Eb2KpSC8uMt9GmzyAEm5Eb1AAAgTjRaXWFjKyFXHZxF3",
    "FCjUJZ1qozm1e8romw216qyfQMaaWKxWsuySnumVCCNe",
    "ENxTEjSQ1YabmUpXAdCgevnHQ9MHdLv8tzFiuiYJqa13",
    "6rYLG55Q9RpsPGvqdPNJs4z5WTxJVatMB8zV3WJhs5EK",
    "Cix2bHfqPcKcM233mzxbLk14kSggUUiz2A87fJtGivXr",
];

const TEMPORAL_TIP_ACCOUNTS: &[&str] = &[
    "TEMPaMeCRFAS9EKF53Jd6KpHxgL47uWLcpFArU1Fanq",
    "noz3jAjPiHuBPqiSPkkugaJDkJscPuRhYnSpbi8UvC4",
    "noz3str9KXfpKknefHji8L1mPgimezaiUyCHYMDv1GE",
    "noz6uoYCDijhu1V7cutCpwxNiSovEwLdRHPwmgCGDNo",
];

const BLOXROUTE_TIP_ACCOUNTS: &[&str] = &[
    "HWEoBxYs7ssKuudEjzjmpfJVX7Dvi7wescFsVx2L5yoY",
    "95cfoy472fcQHaw4tPGBTKpn6ZQnfEPfBgDQx6gcRmRg",
    "3UQUKjhMKaY2S6bjcQD6yHB7utcZt5bfarRCmctpRtUd",
    "FogxVNs6Mm2w9rnGL1vkARSwJxvLE8mujTv3LK8RnUhF",
];

const NODE1_TIP_ACCOUNTS: &[&str] = &[
    "node1PqAa3BWWzUnTHVbw8NJHC874zn9ngAkXjgWEej",
    "node1UzzTxAAeBTpfZkQPJXBAqixsbdth11ba1NXLBG",
    "node1Qm1bV4fwYnCurP8otJ9s5yrkPq7SPZ5uhj3Tsv",
    "node1PUber6SFmSQgvf2ECmXsHP5o3boRSGhvJyPMX1",
];

const FLASHBLOCK_TIP_ACCOUNTS: &[&str] = &[
    "FLaShB3iXXTWE1vu9wQsChUKq3HFtpMAhb8kAh1pf1wi",
    "FLashhsorBmM9dLpuq6qATawcpqk1Y2aqaZfkd48iT3W",
    "FLaSHJNm5dWYzEgnHJWWJP5ccu128Mu61NJLxUf7mUXU",
];

const BLOCKRAZOR_TIP_ACCOUNTS: &[&str] = &[
    "FjmZZrFvhnqqb9ThCuMVnENaM3JGVuGWNyCAxRJcFpg9",
    "6No2i3aawzHsjtThw81iq1EXPJN6rh8eSJCLaYZfKDTG",
    "A9cWowVAiHe9pJfKAj3TJiN9VpbzMUq6E4kEvf5mUT22",
];

const ASTRALANE_TIP_ACCOUNTS: &[&str] = &[
    "astrazznxsGUhWShqgNtAdfrzP2G83DzcWVJDxwV9bF",
    "astra4uejePWneqNaJKuFFA8oonqCE1sqF6b45kDMZm",
    "astra9xWY93QyfG6yM8zwsKsRodscjQ2uU2HKNL5prk",
];

/// 端点常量 (从sol-trade-sdk复制)
const JITO_ENDPOINTS: &[&str] = &[
    "https://ny.mainnet.block-engine.jito.wtf",
    "https://frankfurt.mainnet.block-engine.jito.wtf",
    "https://amsterdam.mainnet.block-engine.jito.wtf",
    "https://slc.mainnet.block-engine.jito.wtf",
    "https://tokyo.mainnet.block-engine.jito.wtf",
    "https://london.mainnet.block-engine.jito.wtf",
    "https://ny.mainnet.block-engine.jito.wtf",
    "https://mainnet.block-engine.jito.wtf",
];

const NEXTBLOCK_ENDPOINTS: &[&str] = &[
    "http://ny.nextblock.io",
    "http://frankfurt.nextblock.io",
    "http://amsterdam.nextblock.io",
    "http://slc.nextblock.io",
    "http://tokyo.nextblock.io",
    "http://london.nextblock.io",
    "http://singapore.nextblock.io",
    "http://frankfurt.nextblock.io",
];

const ZEROSLOT_ENDPOINTS: &[&str] = &[
    "http://ny.0slot.trade",
    "http://de.0slot.trade",
    "http://ams.0slot.trade",
    "http://ny.0slot.trade",
    "http://jp.0slot.trade",
    "http://ams.0slot.trade",
    "http://la.0slot.trade",
    "http://de.0slot.trade",
];

const TEMPORAL_ENDPOINTS: &[&str] = &[
    "http://ewr1.nozomi.temporal.xyz",
    "http://fra2.nozomi.temporal.xyz",
    "http://ams1.nozomi.temporal.xyz",
    "http://ewr1.nozomi.temporal.xyz",
    "http://tyo1.nozomi.temporal.xyz",
    "http://sgp1.nozomi.temporal.xyz",
    "http://pit1.nozomi.temporal.xyz",
    "http://fra2.nozomi.temporal.xyz",
];

const BLOXROUTE_ENDPOINTS: &[&str] = &[
    "https://ny.solana.dex.blxrbdn.com",
    "https://germany.solana.dex.blxrbdn.com",
    "https://amsterdam.solana.dex.blxrbdn.com",
    "https://ny.solana.dex.blxrbdn.com",
    "https://tokyo.solana.dex.blxrbdn.com",
    "https://uk.solana.dex.blxrbdn.com",
    "https://la.solana.dex.blxrbdn.com",
    "https://germany.solana.dex.blxrbdn.com",
];

const NODE1_ENDPOINTS: &[&str] = &[
    "http://ny.node1.me",
    "http://fra.node1.me",
    "http://ams.node1.me",
    "http://ny.node1.me",
    "http://fra.node1.me",
    "http://ams.node1.me",
    "http://ny.node1.me",
    "http://fra.node1.me",
];

const FLASHBLOCK_ENDPOINTS: &[&str] = &[
    "http://ny.flashblock.trade",
    "http://fra.flashblock.trade",
    "http://ams.flashblock.trade",
    "http://slc.flashblock.trade",
    "http://singapore.flashblock.trade",
    "http://london.flashblock.trade",
    "http://ny.flashblock.trade",
    "http://ny.flashblock.trade",
];

const BLOCKRAZOR_ENDPOINTS: &[&str] = &[
    "http://newyork.solana.blockrazor.xyz:443",
    "http://frankfurt.solana.blockrazor.xyz:443",
    "http://amsterdam.solana.blockrazor.xyz:443",
    "http://newyork.solana.blockrazor.xyz:443",
    "http://tokyo.solana.blockrazor.xyz:443",
    "http://frankfurt.solana.blockrazor.xyz:443",
    "http://newyork.solana.blockrazor.xyz:443",
    "http://frankfurt.solana.blockrazor.xyz:443",
];

const ASTRALANE_ENDPOINTS: &[&str] = &[
    "http://ny.gateway.astralane.io/iris",
    "http://fr.gateway.astralane.io/iris",
    "http://ams.gateway.astralane.io/iris",
    "http://ny.gateway.astralane.io/iris",
    "http://jp.gateway.astralane.io/iris",
    "http://ny.gateway.astralane.io/iris",
    "http://lax.gateway.astralane.io/iris",
    "http://lim.gateway.astralane.io/iris",
];

/// 获取端点
fn get_endpoint(swqos_type: SwqosType, region: SwqosRegion) -> String {
    let region_idx = match region {
        SwqosRegion::NewYork => 0,
        SwqosRegion::Frankfurt => 1,
        SwqosRegion::Amsterdam => 2,
        SwqosRegion::SLC => 3,
        SwqosRegion::Tokyo => 4,
        SwqosRegion::London => 5,
        SwqosRegion::LosAngeles => 6,
        SwqosRegion::Default => 7,
    };

    let endpoint = match swqos_type {
        SwqosType::Jito => JITO_ENDPOINTS[region_idx],
        SwqosType::NextBlock => NEXTBLOCK_ENDPOINTS[region_idx],
        SwqosType::ZeroSlot => ZEROSLOT_ENDPOINTS[region_idx],
        SwqosType::Temporal => TEMPORAL_ENDPOINTS[region_idx],
        SwqosType::Bloxroute => BLOXROUTE_ENDPOINTS[region_idx],
        SwqosType::Node1 => NODE1_ENDPOINTS[region_idx],
        SwqosType::FlashBlock => FLASHBLOCK_ENDPOINTS[region_idx],
        SwqosType::BlockRazor => BLOCKRAZOR_ENDPOINTS[region_idx],
        SwqosType::Astralane => ASTRALANE_ENDPOINTS[region_idx],
        SwqosType::Default => "",
    };

    endpoint.to_string()
}

/// 获取随机Tip账户
fn get_random_tip_account(swqos_type: SwqosType) -> Result<String> {
    let mut rng = rand::rng();  // 🔥 修复: rand 0.9 使用 rng() 而非 thread_rng()

    let accounts = match swqos_type {
        SwqosType::Jito => JITO_TIP_ACCOUNTS,
        SwqosType::NextBlock => NEXTBLOCK_TIP_ACCOUNTS,
        SwqosType::ZeroSlot => ZEROSLOT_TIP_ACCOUNTS,
        SwqosType::Temporal => TEMPORAL_TIP_ACCOUNTS,
        SwqosType::Bloxroute => BLOXROUTE_TIP_ACCOUNTS,
        SwqosType::Node1 => NODE1_TIP_ACCOUNTS,
        SwqosType::FlashBlock => FLASHBLOCK_TIP_ACCOUNTS,
        SwqosType::BlockRazor => BLOCKRAZOR_TIP_ACCOUNTS,
        SwqosType::Astralane => ASTRALANE_TIP_ACCOUNTS,
        SwqosType::Default => return Err(anyhow::anyhow!("Default type has no tip accounts")),
    };

    let account_str = accounts.choose(&mut rng)
        .ok_or_else(|| anyhow::anyhow!("No tip accounts available"))?;

    Ok(account_str.to_string())
}

/// SWQOS 服务配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwqosServiceConfig {
    pub name: String,
    pub service_type: SwqosType,
    pub region: SwqosRegion,
    pub api_key: String,
    pub tip_lamports: Option<u64>,
    pub priority: u32,
    pub enabled: bool,
}

impl SwqosServiceConfig {
    pub fn get_endpoint(&self) -> String {
        get_endpoint(self.service_type, self.region)
    }
}

/// SWQOS 客户端 trait (完全参考 sol-trade-sdk 的 SwqosClientTrait)
#[async_trait::async_trait]
pub trait SwqosClientTrait: Send + Sync {
    async fn send_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature>;
    fn get_tip_account(&self) -> Result<String>;
    fn get_swqos_type(&self) -> SwqosType;
}

/// 多 SWQOS 服务管理器
pub struct MultiSwqosManager {
    clients: Vec<Arc<dyn SwqosClientTrait>>,
    config: SwqosConfig,
    results: Arc<RwLock<HashMap<String, SwqosResult>>>,
}

/// SWQOS 发送结果
#[derive(Debug, Clone)]
pub struct SwqosResult {
    pub service_name: String,
    pub signature: Option<Signature>,
    pub success: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// SWQOS 配置
#[derive(Debug, Clone)]
pub struct SwqosConfig {
    pub parallel_send: bool,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub max_tips: usize,  // 最大 tip 数量（避免交易体积过大）
    pub services: Vec<SwqosServiceConfig>,
}

impl SwqosConfig {
    /// 从环境变量加载配置（新格式：每个服务商独立配置）
    pub fn from_env() -> Result<Self> {
        let parallel_send = std::env::var("SWQOS_PARALLEL_SEND")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);

        let timeout_ms = std::env::var("SWQOS_TIMEOUT_MS")
            .unwrap_or_else(|_| "10000".to_string())
            .parse()
            .unwrap_or(10000);

        let max_retries = std::env::var("SWQOS_MAX_RETRIES")
            .unwrap_or_else(|_| "3".to_string())
            .parse()
            .unwrap_or(3);

        let max_tips = std::env::var("SWQOS_MAX_TIPS")
            .unwrap_or_else(|_| "5".to_string())
            .parse()
            .unwrap_or(5);

        let mut services = Vec::new();

        // 加载 Jito
        if let Ok(enabled) = std::env::var("JITO_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(uuid) = std::env::var("JITO_UUID") {
                    let region_str = std::env::var("JITO_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("JITO_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("JITO_PRIORITY")
                        .unwrap_or_else(|_| "1".to_string())
                        .parse()
                        .unwrap_or(1);

                    services.push(SwqosServiceConfig {
                        name: format!("Jito-{:?}", region),
                        service_type: SwqosType::Jito,
                        region,
                        api_key: uuid,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 Jito 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 NextBlock
        if let Ok(enabled) = std::env::var("NEXTBLOCK_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(token) = std::env::var("NEXTBLOCK_TOKEN") {
                    let region_str = std::env::var("NEXTBLOCK_REGION").unwrap_or_else(|_| "Frankfurt".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::Frankfurt);
                    let tip_lamports = std::env::var("NEXTBLOCK_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("NEXTBLOCK_PRIORITY")
                        .unwrap_or_else(|_| "2".to_string())
                        .parse()
                        .unwrap_or(2);

                    services.push(SwqosServiceConfig {
                        name: format!("NextBlock-{:?}", region),
                        service_type: SwqosType::NextBlock,
                        region,
                        api_key: token,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 NextBlock 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 ZeroSlot
        if let Ok(enabled) = std::env::var("ZEROSLOT_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(api_key) = std::env::var("ZEROSLOT_API_KEY") {
                    let region_str = std::env::var("ZEROSLOT_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("ZEROSLOT_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("ZEROSLOT_PRIORITY")
                        .unwrap_or_else(|_| "3".to_string())
                        .parse()
                        .unwrap_or(3);

                    services.push(SwqosServiceConfig {
                        name: format!("ZeroSlot-{:?}", region),
                        service_type: SwqosType::ZeroSlot,
                        region,
                        api_key,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 ZeroSlot 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 Temporal
        if let Ok(enabled) = std::env::var("TEMPORAL_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(api_key) = std::env::var("TEMPORAL_API_KEY") {
                    let region_str = std::env::var("TEMPORAL_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("TEMPORAL_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("TEMPORAL_PRIORITY")
                        .unwrap_or_else(|_| "4".to_string())
                        .parse()
                        .unwrap_or(4);

                    services.push(SwqosServiceConfig {
                        name: format!("Temporal-{:?}", region),
                        service_type: SwqosType::Temporal,
                        region,
                        api_key,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 Temporal 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 Bloxroute
        if let Ok(enabled) = std::env::var("BLOXROUTE_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(auth_header) = std::env::var("BLOXROUTE_AUTH_HEADER") {
                    let region_str = std::env::var("BLOXROUTE_REGION").unwrap_or_else(|_| "Tokyo".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::Tokyo);
                    let tip_lamports = std::env::var("BLOXROUTE_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("BLOXROUTE_PRIORITY")
                        .unwrap_or_else(|_| "5".to_string())
                        .parse()
                        .unwrap_or(5);

                    services.push(SwqosServiceConfig {
                        name: format!("Bloxroute-{:?}", region),
                        service_type: SwqosType::Bloxroute,
                        region,
                        api_key: auth_header,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 Bloxroute 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 Node1
        if let Ok(enabled) = std::env::var("NODE1_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(api_key) = std::env::var("NODE1_API_KEY") {
                    let region_str = std::env::var("NODE1_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("NODE1_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("NODE1_PRIORITY")
                        .unwrap_or_else(|_| "6".to_string())
                        .parse()
                        .unwrap_or(6);

                    services.push(SwqosServiceConfig {
                        name: format!("Node1-{:?}", region),
                        service_type: SwqosType::Node1,
                        region,
                        api_key,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 Node1 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 FlashBlock
        if let Ok(enabled) = std::env::var("FLASHBLOCK_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(api_key) = std::env::var("FLASHBLOCK_API_KEY") {
                    let region_str = std::env::var("FLASHBLOCK_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("FLASHBLOCK_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("FLASHBLOCK_PRIORITY")
                        .unwrap_or_else(|_| "7".to_string())
                        .parse()
                        .unwrap_or(7);

                    services.push(SwqosServiceConfig {
                        name: format!("FlashBlock-{:?}", region),
                        service_type: SwqosType::FlashBlock,
                        region,
                        api_key,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 FlashBlock 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 BlockRazor
        if let Ok(enabled) = std::env::var("BLOCKRAZOR_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(api_key) = std::env::var("BLOCKRAZOR_API_KEY") {
                    let region_str = std::env::var("BLOCKRAZOR_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("BLOCKRAZOR_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("BLOCKRAZOR_PRIORITY")
                        .unwrap_or_else(|_| "8".to_string())
                        .parse()
                        .unwrap_or(8);

                    services.push(SwqosServiceConfig {
                        name: format!("BlockRazor-{:?}", region),
                        service_type: SwqosType::BlockRazor,
                        region,
                        api_key,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 BlockRazor 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        // 加载 Astralane
        if let Ok(enabled) = std::env::var("ASTRALANE_ENABLED") {
            if enabled.to_lowercase() == "true" {
                if let Ok(api_key) = std::env::var("ASTRALANE_API_KEY") {
                    let region_str = std::env::var("ASTRALANE_REGION").unwrap_or_else(|_| "NewYork".to_string());
                    let region = SwqosRegion::from_str(&region_str).unwrap_or(SwqosRegion::NewYork);
                    let tip_lamports = std::env::var("ASTRALANE_TIP_LAMPORTS")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    let priority = std::env::var("ASTRALANE_PRIORITY")
                        .unwrap_or_else(|_| "9".to_string())
                        .parse()
                        .unwrap_or(9);

                    services.push(SwqosServiceConfig {
                        name: format!("Astralane-{:?}", region),
                        service_type: SwqosType::Astralane,
                        region,
                        api_key,
                        tip_lamports,
                        priority,
                        enabled: true,
                    });
                    info!("✅ 加载 Astralane 配置: 区域={:?}, 优先级={}", region, priority);
                }
            }
        }

        if services.is_empty() {
            warn!("⚠️  没有启用任何 SWQOS 服务！");
        } else {
            info!("🎯 总共加载了 {} 个 SWQOS 服务", services.len());
        }

        Ok(Self {
            parallel_send,
            timeout_ms,
            max_retries,
            max_tips,
            services,
        })
    }
}


impl MultiSwqosManager {
    pub fn new(config: SwqosConfig) -> Result<Self> {
        let mut clients: Vec<Arc<dyn SwqosClientTrait>> = Vec::new();

        let mut sorted_services = config.services.clone();
        sorted_services.sort_by_key(|s| s.priority);

        for service_config in &sorted_services {
            if !service_config.enabled {
                continue;
            }

            let client = Self::create_client(service_config)?;
            clients.push(client);
        }

        info!("🚀 多 SWQOS 管理器已初始化");
        info!("   启用服务数量: {}", clients.len());
        info!("   并行发送: {}", config.parallel_send);
        info!("   超时时间: {}ms", config.timeout_ms);

        Ok(Self {
            clients,
            config,
            results: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    fn create_client(service_config: &SwqosServiceConfig) -> Result<Arc<dyn SwqosClientTrait>> {
        let endpoint = service_config.get_endpoint();
        let api_key = service_config.api_key.clone();
        let swqos_type = service_config.service_type;

        let client: Arc<dyn SwqosClientTrait> = match swqos_type {
            SwqosType::Jito => Arc::new(JitoClient::new(endpoint, api_key)),
            SwqosType::NextBlock => Arc::new(NextBlockClient::new(endpoint, api_key)),
            SwqosType::Bloxroute => Arc::new(BloxrouteClient::new(endpoint, api_key)),
            SwqosType::Temporal => Arc::new(TemporalClient::new(endpoint, api_key)),
            SwqosType::ZeroSlot => Arc::new(ZeroSlotClient::new(endpoint, api_key)),
            SwqosType::Node1 => Arc::new(Node1Client::new(endpoint, api_key)),
            SwqosType::FlashBlock => Arc::new(FlashBlockClient::new(endpoint, api_key)),
            SwqosType::BlockRazor => Arc::new(BlockRazorClient::new(endpoint, api_key)),
            SwqosType::Astralane => Arc::new(AstralaneClient::new(endpoint, api_key)),
            SwqosType::Default => {
                return Err(anyhow::anyhow!("Default type is not supported"));
            }
        };

        Ok(client)
    }

    pub async fn send_transaction_race(&self, transaction: &VersionedTransaction) -> Result<SwqosResult> {
        info!("🏁 开始田忌赛马策略发送交易");
        info!("   参与服务数量: {}", self.clients.len());
        info!("   最大重试次数: {}", self.config.max_retries);

        if self.clients.is_empty() {
            return Err(anyhow::anyhow!("没有可用的 SWQOS 服务"));
        }

        let timeout_duration = Duration::from_millis(self.config.timeout_ms);

        // 使用重试逻辑
        let mut last_error = None;
        for attempt in 1..=self.config.max_retries {
            if attempt > 1 {
                info!("🔄 SWQOS 重试 {}/{}", attempt, self.config.max_retries);
            }

            let result = if self.config.parallel_send {
                self.send_parallel(transaction, timeout_duration).await
            } else {
                self.send_sequential(transaction, timeout_duration).await
            };

            match result {
                Ok(res) if res.success => {
                    if attempt > 1 {
                        info!("✅ SWQOS 重试成功 (尝试 {})", attempt);
                    }
                    return Ok(res);
                }
                Ok(res) => {
                    warn!("❌ SWQOS 尝试 {} 失败: {:?}", attempt, res.error);
                    last_error = Some(anyhow::anyhow!("SWQOS failed: {:?}", res.error));
                }
                Err(e) => {
                    warn!("❌ SWQOS 尝试 {} 错误: {}", attempt, e);
                    last_error = Some(e);
                }
            }

            // 如果还有重试机会，等待一小段时间
            if attempt < self.config.max_retries {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("SWQOS 所有重试都失败")))
    }

    async fn send_parallel(&self, transaction: &VersionedTransaction, timeout_duration: Duration) -> Result<SwqosResult> {
        info!("⚡ 使用并行发送策略");

        let mut tasks = Vec::new();

        for (idx, client) in self.clients.iter().enumerate() {
            let client = client.clone();
            let transaction = transaction.clone();
            let service_name = format!("Service-{}", idx);

            let task = tokio::spawn(async move {
                let start = Instant::now();
                match timeout(timeout_duration, client.send_transaction(&transaction)).await {
                    Ok(Ok(signature)) => {
                        let latency = start.elapsed().as_millis() as u64;
                        SwqosResult {
                            service_name,
                            signature: Some(signature),
                            success: true,
                            latency_ms: latency,
                            error: None,
                        }
                    }
                    Ok(Err(e)) => {
                        let latency = start.elapsed().as_millis() as u64;
                        SwqosResult {
                            service_name,
                            signature: None,
                            success: false,
                            latency_ms: latency,
                            error: Some(e.to_string()),
                        }
                    }
                    Err(_) => {
                        let latency = start.elapsed().as_millis() as u64;
                        SwqosResult {
                            service_name,
                            signature: None,
                            success: false,
                            latency_ms: latency,
                            error: Some("Timeout".to_string()),
                        }
                    }
                }
            });

            tasks.push(task);
        }

        let mut first_success: Option<SwqosResult> = None;
        let mut all_results = Vec::new();

        for task in tasks {
            match task.await {
                Ok(result) => {
                    all_results.push(result.clone());
                    if result.success && first_success.is_none() {
                        first_success = Some(result.clone());
                        info!("🏆 第一个成功的服务: {}", result.service_name);
                        break;
                    }
                }
                Err(e) => {
                    error!("任务执行失败: {:?}", e);
                }
            }
        }

        {
            let mut results = self.results.write().await;
            for result in &all_results {
                results.insert(result.service_name.clone(), result.clone());
            }
        }

        if let Some(success_result) = first_success {
            info!("✅ 田忌赛马成功: {} ({}ms)", success_result.service_name, success_result.latency_ms);
            Ok(success_result)
        } else {
            let fastest = all_results.iter().min_by_key(|r| r.latency_ms);
            if let Some(fastest) = fastest {
                warn!("❌ 所有服务都失败，最快失败: {} ({}ms)", fastest.service_name, fastest.latency_ms);
                Ok(fastest.clone())
            } else {
                Err(anyhow::anyhow!("所有 SWQOS 服务都失败"))
            }
        }
    }

    async fn send_sequential(&self, transaction: &VersionedTransaction, timeout_duration: Duration) -> Result<SwqosResult> {
        info!("🔄 使用顺序发送策略");

        for (idx, client) in self.clients.iter().enumerate() {
            let service_name = format!("Service-{}", idx);

            info!("🎯 尝试服务: {}", service_name);

            let start = Instant::now();
            match timeout(timeout_duration, client.send_transaction(transaction)).await {
                Ok(Ok(signature)) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let result = SwqosResult {
                        service_name: service_name.clone(),
                        signature: Some(signature),
                        success: true,
                        latency_ms: latency,
                        error: None,
                    };

                    info!("✅ 顺序发送成功: {} ({}ms)", service_name, latency);
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    let latency = start.elapsed().as_millis() as u64;
                    warn!("❌ 服务 {} 失败: {} ({}ms)", service_name, e, latency);
                }
                Err(_) => {
                    let latency = start.elapsed().as_millis() as u64;
                    warn!("⏰ 服务 {} 超时 ({}ms)", service_name, latency);
                }
            }
        }

        Err(anyhow::anyhow!("所有 SWQOS 服务都失败"))
    }

    /// 获取所有服务商的 tip 指令
    ///
    /// 返回每个启用的服务商的 tip transfer 指令
    /// 用于田忌赛马策略：把所有 tip 都加到同一个交易里
    ///
    /// 📝 交易体积说明：
    ///    1. 单个 tip 指令约 50 bytes（transfer + 3 个账户）
    ///    2. Solana 交易限制 1232 bytes
    ///    3. 当前典型配置 2-3 个服务 ≈ 150 bytes tips
    ///    4. 安全阈值：< 10 个服务（约 500 bytes）
    ///    5. 可用服务总数有限（约 9 个），无需提前优化
    ///
    /// ⚠️ 仅当遇到 "transaction too large" 错误时才需要考虑 ALT 或限流
    pub fn get_all_tip_instructions(
        &self,
        payer: &solana_sdk::pubkey::Pubkey,
    ) -> Result<Vec<(String, solana_sdk::instruction::Instruction)>> {
        use solana_system_interface::instruction::transfer;

        let mut tip_instructions = Vec::new();

        for (client, service_config) in self.clients.iter().zip(&self.config.services) {
            // 获取服务类型
            let swqos_type = client.get_swqos_type();
            debug!("🔍 服务 {}: 类型 = {:?}", service_config.name, swqos_type);

            // 获取 tip 地址
            let tip_address_str = match client.get_tip_account() {
                Ok(addr) => addr,
                Err(e) => {
                    warn!("⚠️  获取服务 {} 的 tip 地址失败: {}", service_config.name, e);
                    continue;
                }
            };

            let tip_address = match tip_address_str.parse::<solana_sdk::pubkey::Pubkey>() {
                Ok(addr) => addr,
                Err(e) => {
                    warn!("⚠️  解析服务 {} 的 tip 地址失败: {}", service_config.name, e);
                    continue;
                }
            };

            // 获取 tip 金额（从配置或使用默认值）
            let tip_lamports = service_config.tip_lamports.unwrap_or(100_000); // 默认 0.0001 SOL

            debug!("💰 服务 {}: tip 地址 = {}, 金额 = {} lamports",
                service_config.name, tip_address, tip_lamports);

            // 创建 transfer 指令
            let tip_instruction = transfer(payer, &tip_address, tip_lamports);

            tip_instructions.push((service_config.name.clone(), tip_instruction));
        }

        // 🔥 按优先级裁剪（取优先级最高的前 max_tips 个）
        if tip_instructions.len() > self.config.max_tips {
            info!("⚠️  服务数量 {} 超过限制 {}，按优先级裁剪",
                tip_instructions.len(), self.config.max_tips);

            // 按 priority 排序（已在初始化时按 priority 排序 services）
            tip_instructions.truncate(self.config.max_tips);

            info!("✅ 裁剪后保留 {} 个高优先级 tip 指令", tip_instructions.len());
        }

        info!("✅ 已生成 {} 个 SWQOS tip 指令", tip_instructions.len());

        Ok(tip_instructions)
    }
}

// ============================================================================
// SWQOS 客户端实现 (完全参考 sol-trade-sdk 的结构)
// ============================================================================

/// Jito 客户端 (参考 sol-trade-sdk/src/swqos/jito.rs)
pub struct JitoClient {
    pub endpoint: String,
    pub auth_token: String,
    pub http_client: Client,
}

impl JitoClient {
    pub fn new(endpoint: String, auth_token: String) -> Self {
        let http_client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(64)
            .tcp_keepalive(Some(Duration::from_secs(1200)))
            .http2_keep_alive_interval(Duration::from_secs(15))
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        Self { endpoint, auth_token, http_client }
    }

    fn serialize_transaction(&self, transaction: &VersionedTransaction) -> Result<String> {
        let serialized = bincode::serialize(transaction)?;
        Ok(STANDARD.encode(serialized))
    }
}

#[async_trait::async_trait]
impl SwqosClientTrait for JitoClient {
    async fn send_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature> {
        let content = self.serialize_transaction(transaction)?;
        let signature = transaction.signatures[0];

        let request_body = serde_json::json!({
            "id": 1,
            "jsonrpc": "2.0",
            "method": "sendTransaction",
            "params": [
                content,
                {
                    "encoding": "base64"
                }
            ]
        });

        let endpoint = if self.auth_token.is_empty() {
            format!("{}/api/v1/transactions", self.endpoint)
        } else {
            format!("{}/api/v1/transactions?uuid={}", self.endpoint, self.auth_token)
        };

        let mut request = self.http_client.post(&endpoint)
            .header("Content-Type", "application/json")
            .json(&request_body);

        if !self.auth_token.is_empty() {
            request = request.header("x-jito-auth", &self.auth_token);
        }

        let response = request.send().await?;
        let response_text = response.text().await?;

        if let Ok(response_json) = serde_json::from_str::<serde_json::Value>(&response_text) {
            if response_json.get("result").is_some() {
                return Ok(signature);
            } else if let Some(error) = response_json.get("error") {
                return Err(anyhow::anyhow!("Jito error: {:?}", error));
            }
        }

        Err(anyhow::anyhow!("Jito failed: {}", response_text))
    }

    fn get_tip_account(&self) -> Result<String> {
        get_random_tip_account(SwqosType::Jito)
    }

    fn get_swqos_type(&self) -> SwqosType {
        SwqosType::Jito
    }
}

/// NextBlock 客户端
pub struct NextBlockClient {
    pub endpoint: String,
    pub auth_token: String,
    pub http_client: Client,
}

impl NextBlockClient {
    pub fn new(endpoint: String, auth_token: String) -> Self {
        let endpoint = if endpoint.ends_with("/api/v2/submit") {
            endpoint
        } else {
            format!("{}/api/v2/submit", endpoint.trim_end_matches('/'))
        };
        let http_client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(64)
            .tcp_keepalive(Some(Duration::from_secs(1200)))
            .http2_keep_alive_interval(Duration::from_secs(15))
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        Self { endpoint, auth_token, http_client }
    }

    fn serialize_transaction(&self, transaction: &VersionedTransaction) -> Result<String> {
        let serialized = bincode::serialize(transaction)?;
        Ok(STANDARD.encode(serialized))
    }
}

#[async_trait::async_trait]
impl SwqosClientTrait for NextBlockClient {
    async fn send_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature> {
        let content = self.serialize_transaction(transaction)?;
        let signature = transaction.signatures[0];

        let request_body = serde_json::json!({
            "transaction": {
                "content": content
            },
            "frontRunningProtection": false
        });

        let response = self.http_client.post(&self.endpoint)
            .header("Authorization", &self.auth_token)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        let response_text = response.text().await?;

        if let Ok(response_json) = serde_json::from_str::<serde_json::Value>(&response_text) {
            if response_json.get("signature").is_some() {
                return Ok(signature);
            } else if let Some(reason) = response_json.get("reason") {
                return Err(anyhow::anyhow!("NextBlock error: {:?}", reason));
            }
        }

        Err(anyhow::anyhow!("NextBlock failed: {}", response_text))
    }

    fn get_tip_account(&self) -> Result<String> {
        get_random_tip_account(SwqosType::NextBlock)
    }

    fn get_swqos_type(&self) -> SwqosType {
        SwqosType::NextBlock
    }
}

/// Bloxroute 客户端
pub struct BloxrouteClient {
    pub endpoint: String,
    pub auth_token: String,
    pub http_client: Client,
}

impl BloxrouteClient {
    pub fn new(endpoint: String, auth_token: String) -> Self {
        let http_client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(64)
            .tcp_keepalive(Some(Duration::from_secs(1200)))
            .http2_keep_alive_interval(Duration::from_secs(15))
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        Self { endpoint, auth_token, http_client }
    }

    fn serialize_transaction(&self, transaction: &VersionedTransaction) -> Result<String> {
        let serialized = bincode::serialize(transaction)?;
        Ok(STANDARD.encode(serialized))
    }
}

#[async_trait::async_trait]
impl SwqosClientTrait for BloxrouteClient {
    async fn send_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature> {
        let content = self.serialize_transaction(transaction)?;
        let signature = transaction.signatures[0];

        let request_body = serde_json::json!({
            "transaction": {
                "content": content,
            },
            "frontRunningProtection": false,
            "useStakedRPCs": true,
        });

        let endpoint = format!("{}/api/v2/submit", self.endpoint);
        let response = self.http_client.post(&endpoint)
            .header("Content-Type", "application/json")
            .header("Authorization", &self.auth_token)
            .json(&request_body)
            .send()
            .await?;

        let response_text = response.text().await?;

        if let Ok(response_json) = serde_json::from_str::<serde_json::Value>(&response_text) {
            if response_json.get("result").is_some() {
                return Ok(signature);
            } else if let Some(error) = response_json.get("error") {
                return Err(anyhow::anyhow!("Bloxroute error: {:?}", error));
            }
        }

        Err(anyhow::anyhow!("Bloxroute failed: {}", response_text))
    }

    fn get_tip_account(&self) -> Result<String> {
        get_random_tip_account(SwqosType::Bloxroute)
    }

    fn get_swqos_type(&self) -> SwqosType {
        SwqosType::Bloxroute
    }
}

// 使用宏简化其他客户端实现
macro_rules! impl_simple_swqos_client {
    ($client_name:ident, $swqos_type:expr) => {
        pub struct $client_name {
            pub endpoint: String,
            pub auth_token: String,
            pub http_client: Client,
        }

        impl $client_name {
            pub fn new(endpoint: String, auth_token: String) -> Self {
                let http_client = Client::builder()
                    .pool_idle_timeout(Duration::from_secs(60))
                    .pool_max_idle_per_host(64)
                    .tcp_keepalive(Some(Duration::from_secs(1200)))
                    .http2_keep_alive_interval(Duration::from_secs(15))
                    .timeout(Duration::from_secs(10))
                    .connect_timeout(Duration::from_secs(5))
                    .build()
                    .unwrap();
                Self { endpoint, auth_token, http_client }
            }

            fn serialize_transaction(&self, transaction: &VersionedTransaction) -> Result<String> {
                let serialized = bincode::serialize(transaction)?;
                Ok(STANDARD.encode(serialized))
            }
        }

        #[async_trait::async_trait]
        impl SwqosClientTrait for $client_name {
            async fn send_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature> {
                let content = self.serialize_transaction(transaction)?;
                let signature = transaction.signatures[0];

                let request_body = serde_json::json!({
                    "transaction": {
                        "content": content
                    }
                });

                let response = self.http_client.post(&self.endpoint)
                    .header("Authorization", &self.auth_token)
                    .header("Content-Type", "application/json")
                    .json(&request_body)
                    .send()
                    .await?;

                let response_text = response.text().await?;

                if let Ok(response_json) = serde_json::from_str::<serde_json::Value>(&response_text) {
                    if response_json.get("signature").is_some() || response_json.get("result").is_some() {
                        return Ok(signature);
                    } else if let Some(error) = response_json.get("error").or_else(|| response_json.get("reason")) {
                        return Err(anyhow::anyhow!("{} error: {:?}", stringify!($client_name), error));
                    }
                }

                Err(anyhow::anyhow!("{} failed: {}", stringify!($client_name), response_text))
            }

            fn get_tip_account(&self) -> Result<String> {
                get_random_tip_account($swqos_type)
            }

            fn get_swqos_type(&self) -> SwqosType {
                $swqos_type
            }
        }
    };
}

impl_simple_swqos_client!(TemporalClient, SwqosType::Temporal);
impl_simple_swqos_client!(ZeroSlotClient, SwqosType::ZeroSlot);
impl_simple_swqos_client!(Node1Client, SwqosType::Node1);
impl_simple_swqos_client!(FlashBlockClient, SwqosType::FlashBlock);
impl_simple_swqos_client!(BlockRazorClient, SwqosType::BlockRazor);
impl_simple_swqos_client!(AstralaneClient, SwqosType::Astralane);
