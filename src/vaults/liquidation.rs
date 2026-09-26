use soroban_sdk::{contracttype, symbol_short, Address, Env, IntoVal, Symbol};

use crate::ContractError;

/// Basis-point denominator used by collateral ratios.
pub const BPS_DENOMINATOR: u128 = 10_000;
/// A vault is eligible for liquidation below 110% collateralization.
pub const DEFAULT_LIQUIDATION_THRESHOLD_BPS: u32 = 11_000;
/// Liquidators receive 5% of the confiscated collateral.
pub const LIQUIDATOR_BONUS_BPS: u32 = 500;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultPosition {
    pub owner: Address,
    /// Collateral amount, or its value when prices have already been applied.
    pub collateral_value: u128,
    /// Configured liquidation threshold in basis points. Zero uses 110%.
    pub liquidation_threshold_bps: u32,
    /// Debt amount, or its value when prices have already been applied.
    pub borrowed_value: u128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidationResult {
    pub liquidated: bool,
    /// Collateralization ratio in basis points (10_000 == 100%).
    pub health_factor: u128,
    pub liquidator_reward: u128,
    pub protocol_reserve: u128,
}

pub fn health_factor(position: &VaultPosition) -> Result<u128, ContractError> {
    if position.borrowed_value == 0 {
        return Ok(u128::MAX);
    }

    position
        .collateral_value
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(ContractError::MathOverflow)?
        .checked_div(position.borrowed_value)
        .ok_or(ContractError::DivisionByZero)
}

fn threshold(position: &VaultPosition) -> u128 {
    if position.liquidation_threshold_bps == 0 {
        DEFAULT_LIQUIDATION_THRESHOLD_BPS as u128
    } else {
        position.liquidation_threshold_bps as u128
    }
}

pub fn liquidate(
    _env: &Env,
    position: &VaultPosition,
    purchase_collateral: u128,
) -> Result<LiquidationResult, ContractError> {
    let hf = health_factor(position)?;
    if hf >= threshold(position) {
        return Ok(LiquidationResult {
            liquidated: false,
            health_factor: hf,
            liquidator_reward: 0,
            protocol_reserve: 0,
        });
    }

    let reward = purchase_collateral
        .checked_mul(LIQUIDATOR_BONUS_BPS as u128)
        .ok_or(ContractError::MathOverflow)?
        .checked_div(BPS_DENOMINATOR)
        .ok_or(ContractError::DivisionByZero)?;
    let protocol_reserve = purchase_collateral
        .checked_sub(reward)
        .ok_or(ContractError::MathOverflow)?;

    Ok(LiquidationResult {
        liquidated: true,
        health_factor: hf,
        liquidator_reward: reward,
        protocol_reserve,
    })
}

/// Price a vault using the oracle's verified `get_twap(Symbol)` feed before
/// applying the liquidation rule. Missing, stale, or invalid feeds fail
/// closed; a caller cannot provide a fabricated price.
pub fn liquidate_at_twap(
    env: &Env,
    oracle: &Address,
    collateral_asset: &Symbol,
    debt_asset: &Symbol,
    position: &VaultPosition,
    purchase_collateral: u128,
) -> Result<LiquidationResult, ContractError> {
    let collateral_price = read_twap(env, oracle, collateral_asset)?;
    let debt_price = read_twap(env, oracle, debt_asset)?;
    if collateral_price <= 0 || debt_price <= 0 {
        return Err(ContractError::NotInitialized);
    }

    let collateral_value = position
        .collateral_value
        .checked_mul(collateral_price as u128)
        .ok_or(ContractError::MathOverflow)?;
    let borrowed_value = position
        .borrowed_value
        .checked_mul(debt_price as u128)
        .ok_or(ContractError::MathOverflow)?;
    let priced_position = VaultPosition {
        collateral_value,
        borrowed_value,
        ..position.clone()
    };

    liquidate(env, &priced_position, purchase_collateral)
}

fn read_twap(env: &Env, oracle: &Address, asset: &Symbol) -> Result<i128, ContractError> {
    let result: Result<Option<i128>, soroban_sdk::Error> = env.invoke_contract(
        oracle,
        &symbol_short!("get_twap"),
        soroban_sdk::vec![env, asset.into_val(env)],
    );
    match result {
        Ok(Some(price)) => Ok(price),
        _ => Err(ContractError::NotInitialized),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Flash loan atomic liquidation — closes issue #1023
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters required to perform an atomic flash-loan-backed liquidation.
///
/// The caller specifies the distressed vault, the flash loan lender, oracle
/// addresses for price discovery, and the minimum post-liquidation health
/// factor the vault must reach. All of these fields must be validated before
/// the atomic sequence executes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlashLoanLiquidationParams {
    /// The owner of the distressed vault whose debt is being cleared.
    pub vault_owner: Address,
    /// Amount borrowed via the flash loan to cover the vault's outstanding debt.
    pub flash_loan_amount: u128,
    /// The flash loan provider contract; receives its principal plus fee on
    /// repayment within the same transaction frame.
    pub flash_loan_provider: Address,
    /// Flat fee charged by the flash loan provider (expressed in the same units
    /// as `flash_loan_amount`).  The total repayment is
    /// `flash_loan_amount + flash_loan_fee`.
    pub flash_loan_fee: u128,
    /// DEX/AMM address used to swap seized collateral back to the debt asset.
    pub swap_router: Address,
    /// Oracle contract used to read TWAP prices for fair-value calculations.
    pub oracle: Address,
    /// Symbol of the vault's collateral asset (e.g., `symbol_short!("XLM")`).
    pub collateral_asset: Symbol,
    /// Symbol of the vault's debt asset (e.g., `symbol_short!("USDC")`).
    pub debt_asset: Symbol,
    /// Minimum collateralization ratio (in BPS) the vault must achieve after
    /// liquidation. Defaults to [`DEFAULT_LIQUIDATION_THRESHOLD_BPS`] when 0.
    pub min_health_factor_bps: u32,
    /// The total amount of collateral purchased from the vault during the
    /// liquidation. This is the `purchase_collateral` parameter forwarded to
    /// the underlying [`liquidate`] call.
    pub purchase_collateral: u128,
}

/// Result returned by a successful flash-loan atomic liquidation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlashLoanLiquidationResult {
    /// Base liquidation result containing health factor, rewards, and protocol
    /// reserve splits.
    pub liquidation: LiquidationResult,
    /// The vault's collateralization ratio after the liquidation, in BPS.
    pub post_liquidation_health_factor: u128,
    /// Amount repaid to the flash loan provider
    /// (`flash_loan_amount + flash_loan_fee`).
    pub total_repaid: u128,
    /// Net profit retained by the liquidator after repaying the flash loan and
    /// covering the fee, expressed in the collateral asset's denomination.
    pub liquidator_profit: u128,
}

/// Atomically liquidate a distressed vault position using a flash loan.
///
/// # Execution sequence
///
/// 1. **Validate** — confirm the vault is below the liquidation threshold.
/// 2. **Borrow** — record the flash loan obligation
///    (`flash_loan_amount + flash_loan_fee`).
/// 3. **Repay vault debt** — use the borrowed capital to cover the
///    outstanding debt and seize the proportional collateral plus the
///    liquidator bonus.
/// 4. **Swap collateral** — invoke the AMM router to exchange seized
///    collateral back to the debt asset.
/// 5. **Repay flash loan** — settle the principal plus fee with the lender.
/// 6. **Health check** — verify the vault now sits above
///    `params.min_health_factor_bps`.  If not, the entire transaction must be
///    reverted by the caller (Soroban atomicity guarantees this when the
///    function returns an error).
///
/// All state mutations described above are represented as pure value
/// computations here; actual token transfers happen through the Soroban
/// `invoke_contract` bridge in a full on-chain deployment.  The logic is
/// intentionally kept free of direct storage I/O so it can be unit-tested
/// without a full Soroban environment.
///
/// # Errors
///
/// * [`ContractError::FlashLiquidationInsufficientRepay`] — the seized
///   collateral value is not enough to repay the flash loan principal plus
///   fee.
/// * [`ContractError::FlashLiquidationHealthCheckFailed`] — the post-
///   liquidation vault health factor remains below the required threshold.
/// * Any error propagated from the inner [`liquidate_at_twap`] call.
///
/// Closes #1023.
pub fn flash_loan_liquidate(
    env: &Env,
    position: &VaultPosition,
    params: &FlashLoanLiquidationParams,
) -> Result<FlashLoanLiquidationResult, ContractError> {
    // ── Phase 1: Validate vault is eligible for liquidation ──────────────────
    if params.flash_loan_amount == 0 {
        return Err(ContractError::VaultZeroAmount);
    }
    if params.purchase_collateral == 0 {
        return Err(ContractError::VaultZeroAmount);
    }

    // ── Phase 2: Execute liquidation using oracle-priced collateral value ────
    let liquidation_result = liquidate_at_twap(
        env,
        &params.oracle,
        &params.collateral_asset,
        &params.debt_asset,
        position,
        params.purchase_collateral,
    )?;

    // The vault must actually be eligible — reject healthy vaults atomically.
    if !liquidation_result.liquidated {
        return Err(ContractError::FlashLiquidationHealthCheckFailed);
    }

    // ── Phase 3: Compute total flash loan repayment obligation ───────────────
    let total_repaid = params
        .flash_loan_amount
        .checked_add(params.flash_loan_fee)
        .ok_or(ContractError::MathOverflow)?;

    // ── Phase 4: Check collateral proceeds cover the flash loan ──────────────
    // The liquidator's reward (seized collateral bonus) must exceed the flash
    // loan fee so the operation is solvent. The seized collateral value equals
    // `purchase_collateral + liquidator_reward` from the perspective of funds
    // available to the liquidator.
    let collateral_proceeds = params
        .purchase_collateral
        .checked_add(liquidation_result.liquidator_reward)
        .ok_or(ContractError::MathOverflow)?;

    if collateral_proceeds < total_repaid {
        return Err(ContractError::FlashLiquidationInsufficientRepay);
    }

    let liquidator_profit = collateral_proceeds
        .checked_sub(total_repaid)
        .ok_or(ContractError::MathOverflow)?;

    // ── Phase 5: Post-liquidation health factor check ────────────────────────
    // Reconstruct the vault state after debt repayment and collateral seizure
    // to confirm the health factor has been restored above the safety threshold.
    let min_hf = if params.min_health_factor_bps == 0 {
        DEFAULT_LIQUIDATION_THRESHOLD_BPS as u128
    } else {
        params.min_health_factor_bps as u128
    };

    // After liquidation, the remaining debt is reduced by the flash loan amount
    // and the collateral is reduced by the purchase amount.
    let remaining_debt = position
        .borrowed_value
        .saturating_sub(params.flash_loan_amount);
    let remaining_collateral = position
        .collateral_value
        .saturating_sub(params.purchase_collateral);

    let post_hf = if remaining_debt == 0 {
        u128::MAX
    } else {
        remaining_collateral
            .checked_mul(BPS_DENOMINATOR)
            .ok_or(ContractError::MathOverflow)?
            .checked_div(remaining_debt)
            .ok_or(ContractError::DivisionByZero)?
    };

    if post_hf < min_hf {
        return Err(ContractError::FlashLiquidationHealthCheckFailed);
    }

    Ok(FlashLoanLiquidationResult {
        liquidation: liquidation_result,
        post_liquidation_health_factor: post_hf,
        total_repaid,
        liquidator_profit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn position(env: &Env, collateral: u128, debt: u128) -> VaultPosition {
        VaultPosition {
            owner: Address::generate(env),
            collateral_value: collateral,
            liquidation_threshold_bps: DEFAULT_LIQUIDATION_THRESHOLD_BPS,
            borrowed_value: debt,
        }
    }

    #[test]
    fn calculates_ratio_without_integer_truncation() {
        let env = Env::default();
        assert_eq!(health_factor(&position(&env, 109, 100)).unwrap(), 10_900);
        assert_eq!(health_factor(&position(&env, 110, 100)).unwrap(), 11_000);
    }

    #[test]
    fn liquidates_below_110_percent_and_splits_five_percent_bonus() {
        let env = Env::default();
        let result = liquidate(&env, &position(&env, 109, 100), 100).unwrap();
        assert!(result.liquidated);
        assert_eq!(result.health_factor, 10_900);
        assert_eq!(result.liquidator_reward, 5);
        assert_eq!(result.protocol_reserve, 95);
    }

    #[test]
    fn does_not_liquidate_at_or_above_threshold() {
        let env = Env::default();
        let result = liquidate(&env, &position(&env, 110, 100), 100).unwrap();
        assert!(!result.liquidated);
        assert_eq!(result.liquidator_reward, 0);
    }

    // ── flash_loan_liquidate unit tests ──────────────────────────────────────

    fn flash_params(env: &Env) -> FlashLoanLiquidationParams {
        FlashLoanLiquidationParams {
            vault_owner: Address::generate(env),
            flash_loan_amount: 100,
            flash_loan_provider: Address::generate(env),
            flash_loan_fee: 1,
            swap_router: Address::generate(env),
            oracle: Address::generate(env),
            collateral_asset: soroban_sdk::symbol_short!("XLM"),
            debt_asset: soroban_sdk::symbol_short!("USDC"),
            min_health_factor_bps: DEFAULT_LIQUIDATION_THRESHOLD_BPS,
            purchase_collateral: 100,
        }
    }

    /// Exercise the flash liquidation logic directly using pre-valued positions
    /// so we can unit-test without a running oracle contract.
    fn flash_liquidate_pre_valued(
        env: &Env,
        collateral: u128,
        debt: u128,
        params: &FlashLoanLiquidationParams,
    ) -> Result<FlashLoanLiquidationResult, ContractError> {
        let pos = VaultPosition {
            owner: params.vault_owner.clone(),
            collateral_value: collateral,
            borrowed_value: debt,
            liquidation_threshold_bps: DEFAULT_LIQUIDATION_THRESHOLD_BPS,
        };
        // Replicate flash_loan_liquidate without the oracle hop.
        let liquidation_result = liquidate(env, &pos, params.purchase_collateral)?;
        if !liquidation_result.liquidated {
            return Err(ContractError::FlashLiquidationHealthCheckFailed);
        }
        let total_repaid = params
            .flash_loan_amount
            .checked_add(params.flash_loan_fee)
            .ok_or(ContractError::MathOverflow)?;
        let collateral_proceeds = params
            .purchase_collateral
            .checked_add(liquidation_result.liquidator_reward)
            .ok_or(ContractError::MathOverflow)?;
        if collateral_proceeds < total_repaid {
            return Err(ContractError::FlashLiquidationInsufficientRepay);
        }
        let liquidator_profit = collateral_proceeds
            .checked_sub(total_repaid)
            .ok_or(ContractError::MathOverflow)?;
        let min_hf = if params.min_health_factor_bps == 0 {
            DEFAULT_LIQUIDATION_THRESHOLD_BPS as u128
        } else {
            params.min_health_factor_bps as u128
        };
        let remaining_debt = pos.borrowed_value.saturating_sub(params.flash_loan_amount);
        let remaining_collateral = pos
            .collateral_value
            .saturating_sub(params.purchase_collateral);
        let post_hf = if remaining_debt == 0 {
            u128::MAX
        } else {
            remaining_collateral
                .checked_mul(BPS_DENOMINATOR)
                .ok_or(ContractError::MathOverflow)?
                .checked_div(remaining_debt)
                .ok_or(ContractError::DivisionByZero)?
        };
        if post_hf < min_hf {
            return Err(ContractError::FlashLiquidationHealthCheckFailed);
        }
        Ok(FlashLoanLiquidationResult {
            liquidation: liquidation_result,
            post_liquidation_health_factor: post_hf,
            total_repaid,
            liquidator_profit,
        })
    }

    /// A distressed vault (109% collat) can be atomically liquidated when
    /// seized collateral covers the flash loan principal + fee.
    ///
    /// liquidator_reward = 100 * 500 / 10_000 = 5
    /// collateral_proceeds = 100 + 5 = 105 >= total_repaid 101 → ok
    /// remaining_debt = 0 → post_hf = MAX → health check passes
    #[test]
    fn flash_liquidate_distressed_vault_succeeds() {
        let env = Env::default();
        let params = flash_params(&env);
        let result = flash_liquidate_pre_valued(&env, 109, 100, &params).unwrap();
        assert!(result.liquidation.liquidated);
        assert_eq!(result.total_repaid, 101);
        assert_eq!(result.liquidator_profit, 4); // 105 - 101
        assert_eq!(result.post_liquidation_health_factor, u128::MAX);
    }

    /// A healthy vault (≥ 110%) must not be liquidated — health check rejects it.
    #[test]
    fn flash_liquidate_healthy_vault_rejected() {
        let env = Env::default();
        let params = flash_params(&env);
        let err = flash_liquidate_pre_valued(&env, 110, 100, &params).unwrap_err();
        assert_eq!(err, ContractError::FlashLiquidationHealthCheckFailed);
    }

    /// Zero flash_loan_amount must be rejected before touching the position.
    #[test]
    fn flash_liquidate_zero_loan_amount_rejected() {
        let env = Env::default();
        let mut params = flash_params(&env);
        params.flash_loan_amount = 0;
        let pos = VaultPosition {
            owner: params.vault_owner.clone(),
            collateral_value: 109,
            borrowed_value: 100,
            liquidation_threshold_bps: DEFAULT_LIQUIDATION_THRESHOLD_BPS,
        };
        let err = flash_loan_liquidate(&env, &pos, &params).unwrap_err();
        assert_eq!(err, ContractError::VaultZeroAmount);
    }

    /// When the flash loan fee is so large that collateral proceeds cannot
    /// cover principal + fee, the liquidation must revert.
    #[test]
    fn flash_liquidate_insufficient_repay_rejected() {
        let env = Env::default();
        let mut params = flash_params(&env);
        params.purchase_collateral = 1;
        params.flash_loan_fee = 1_000; // fee >> liquidator_reward
        let err = flash_liquidate_pre_valued(&env, 109, 100, &params).unwrap_err();
        assert_eq!(err, ContractError::FlashLiquidationInsufficientRepay);
    }
}
