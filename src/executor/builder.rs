/// 交易构建器
pub struct TransactionBuilder;

impl TransactionBuilder {
    pub fn new() -> Self {
        Self
    }

    /// 估算买入可获得的 token 数量
    ///
    /// 完全对齐 sol-trade-sdk 的 BondingCurveAccount::get_buy_price 实现
    /// 参考: sol-trade-sdk/src/common/bonding_curve.rs:117-141
    pub fn estimate_buy_token_amount(
        &self,
        virtual_token_reserves: u64,
        virtual_sol_reserves: u64,
        sol_amount: u64,
    ) -> u64 {
        if sol_amount == 0 {
            return 0;
        }

        if virtual_sol_reserves == 0 || virtual_token_reserves == 0 {
            return 0;
        }

        // Calculate the product of virtual reserves using u128 to avoid overflow
        let n: u128 = (virtual_sol_reserves as u128) * (virtual_token_reserves as u128);

        // Calculate the new virtual sol reserves after the purchase
        let i: u128 = (virtual_sol_reserves as u128) + (sol_amount as u128);

        // Calculate the new virtual token reserves after the purchase
        let r: u128 = n / i + 1;

        // Calculate the amount of tokens to be purchased
        let s: u128 = (virtual_token_reserves as u128) - r;

        // 🔥 修复: 安全转换，避免溢出
        // Convert back to u64 with overflow protection
        s.min(u64::MAX as u128) as u64
    }

    /// 估算卖出可获得的 SOL 数量
    ///
    /// 完全对齐 sol-trade-sdk 的 BondingCurveAccount::get_sell_price 实现
    /// 🔥 修复: 使用正确的费率 FEE_BASIS_POINTS=95 + CREATOR_FEE=30
    /// 参考: sol-trade-sdk/src/common/bonding_curve.rs:152-169
    pub fn estimate_sell_sol_amount(
        &self,
        virtual_token_reserves: u64,
        virtual_sol_reserves: u64,
        token_amount: u64,
    ) -> u64 {
        if token_amount == 0 {
            return 0;
        }

        if virtual_sol_reserves == 0 || virtual_token_reserves == 0 {
            return 0;
        }

        // 🔥 修复: PumpFun 卖出费率（对齐 sol-trade-sdk）
        // FEE_BASIS_POINTS = 95 (0.95%)
        // CREATOR_FEE = 30 (0.30%)
        // 总费率 = 125 bps (1.25%)
        const FEE_BASIS_POINTS: u128 = 95;
        const CREATOR_FEE: u128 = 30;
        let total_fee_basis_points = FEE_BASIS_POINTS + CREATOR_FEE;

        // Calculate the proportional amount of virtual sol reserves to be received using u128
        let n: u128 = ((token_amount as u128) * (virtual_sol_reserves as u128))
            / ((virtual_token_reserves as u128) + (token_amount as u128));

        // Calculate the fee amount in the same units
        let a: u128 = (n * total_fee_basis_points) / 10000;

        // 🔥 修复: 安全转换，避免溢出
        // Return the net amount after deducting the fee, converting back to u64
        let result = n.saturating_sub(a);
        result.min(u64::MAX as u128) as u64
    }
}
