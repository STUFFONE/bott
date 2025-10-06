// lib.rs - 导出公共接口供集成测试使用

pub mod types;
pub mod advanced_metrics;
pub mod advanced_filter;
pub mod dynamic_strategy;
pub mod aggregator;
pub mod strategy;
pub mod config;
pub mod grpc;
pub mod executor;
pub mod position;
pub mod momentum_decay;
pub mod monitor;
pub mod swqos;

// 重新导出常用类型
pub use types::{PumpFunEvent, PumpFunEventType, WindowMetrics, SniperEvent};
pub use advanced_metrics::{AdvancedMetrics, AdvancedMetricsCalculator};
pub use advanced_filter::{AdvancedEventFilter, AdvancedFilterConfig};
pub use dynamic_strategy::{DynamicStrategyEngine, DynamicStrategyConfig};
