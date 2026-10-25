#![no_std]
//! Lending vault that takes NOVA State-Bound Assets as collateral.
//!
//! The vault never trusts a market on its own. Every deposit and borrow first calls
//! NOVA's `is_eligible()`, so a market that is not `Active` (including one with a
//! stale checkpoint) cannot add risk.
//!
//! Scope (MVP):
//! - Lenders supply the loan token. There is no interest, so supply is principal only.
//! - Borrowers post SBA collateral per market and borrow the loan token against it.
//! - Collateral value = amount * oracle price / PRICE_SCALE. Borrow limit = value * LTV.
//! - No liquidation. If a price falls, debt can exceed the limit. See the README.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error, token,
    Address, Env,
};

/// Oracle prices are read at 7 decimals (10_000_000 = 1.0).
pub const PRICE_SCALE: i128 = 10_000_000;
const BPS: i128 = 10_000;
/// Hard cap on loan-to-value. Above this the vault refuses to start.
pub const MAX_LTV_BPS: u32 = 8_000;
/// Hard cap on the liquidator's collateral bonus.
pub const MAX_BONUS_BPS: u32 = 2_000;
/// A single liquidation can repay at most this share of the outstanding debt.
pub const CLOSE_FACTOR_BPS: i128 = 5_000;

const LEDGER_DAY: u32 = 17_280;
const TTL_THRESHOLD: u32 = LEDGER_DAY * 7;
const TTL_EXTEND: u32 = LEDGER_DAY * 30;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidParams = 3,
    MarketNotEligible = 4,
    BorrowLimitExceeded = 5,
    InsufficientLiquidity = 6,
    InsufficientCollateral = 7,
    InsufficientSupply = 8,
    InvalidPrice = 9,
    NotLiquidatable = 10,
}

/// Same XDR shape as NOVA's `MarketState`.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MarketState {
    Discovery,
    Ready,
    Active,
    Degraded,
    Paused,
}

/// Same XDR shape as NOVA's `Observation`.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    pub health: MarketState,
    pub price: i128,
    pub oracle_timestamp: u64,
    pub spread_bps: i128,
    pub depth: i128,
}

/// The subset of NOVA's interface the vault uses.
#[contractclient(name = "NovaClient")]
pub trait NovaMarket {
    fn is_eligible(env: Env, market_id: u64) -> bool;
    fn state(env: Env, market_id: u64) -> MarketState;
    fn observe(env: Env, market_id: u64) -> Observation;
    fn transfer(env: Env, market_id: u64, from: Address, to: Address, amount: i128);
    fn settle_transfer(
        env: Env,
        market_id: u64,
        settler: Address,
        from: Address,
        to: Address,
        amount: i128,
    );
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub nova: Address,
    pub loan_token: Address,
    pub ltv_bps: u32,
    pub liquidation_bonus_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub collateral: i128,
    pub debt: i128,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Config,
    Supplied(Address),
    Collateral(Address, u64),
    Debt(Address, u64),
}

#[contract]
pub struct NovaVault;

#[contractimpl]
impl NovaVault {
    pub fn initialize(
        env: Env,
        nova: Address,
        loan_token: Address,
        ltv_bps: u32,
        liquidation_bonus_bps: u32,
    ) {
        if env.storage().instance().has(&DataKey::Config) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        if ltv_bps == 0 || ltv_bps > MAX_LTV_BPS || liquidation_bonus_bps > MAX_BONUS_BPS {
            panic_with_error!(&env, Error::InvalidParams);
        }
        env.storage().instance().set(
            &DataKey::Config,
            &Config {
                nova,
                loan_token,
                ltv_bps,
                liquidation_bonus_bps,
            },
        );
    }

    pub fn config(env: Env) -> Config {
        load_config(&env)
    }

    /// Lender deposits the loan token into the pool.
    pub fn supply(env: Env, lender: Address, amount: i128) {
        lender.require_auth();
        require_positive(&env, amount);
        let cfg = load_config(&env);
        token::Client::new(&env, &cfg.loan_token).transfer(
            &lender,
            &env.current_contract_address(),
            &amount,
        );
        let key = DataKey::Supplied(lender.clone());
        let supplied: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        set_persistent(&env, &key, &(supplied + amount));
    }

    /// Lender withdraws principal, limited by the pool's loan-token cash.
    pub fn withdraw_supply(env: Env, lender: Address, amount: i128) {
        lender.require_auth();
        require_positive(&env, amount);
        let cfg = load_config(&env);
        let key = DataKey::Supplied(lender.clone());
        let supplied: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if supplied < amount {
            panic_with_error!(&env, Error::InsufficientSupply);
        }
        let loan = token::Client::new(&env, &cfg.loan_token);
        if loan.balance(&env.current_contract_address()) < amount {
            panic_with_error!(&env, Error::InsufficientLiquidity);
        }
        set_persistent(&env, &key, &(supplied - amount));
        loan.transfer(&env.current_contract_address(), &lender, &amount);
    }

    /// Post SBA collateral. Requires the market to be eligible, so Degraded or Paused markets are refused.
    pub fn deposit_collateral(env: Env, user: Address, market_id: u64, amount: i128) {
        user.require_auth();
        require_positive(&env, amount);
        let cfg = load_config(&env);
        require_eligible(&env, &cfg, market_id);

        let vault = env.current_contract_address();
        NovaClient::new(&env, &cfg.nova).transfer(&market_id, &user, &vault, &amount);

        let key = DataKey::Collateral(user.clone(), market_id);
        let collateral: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        set_persistent(&env, &key, &(collateral + amount));
    }

    /// Borrow the loan token against posted collateral. Requires the market to be eligible.
    pub fn borrow(env: Env, user: Address, market_id: u64, amount: i128) {
        user.require_auth();
        require_positive(&env, amount);
        let cfg = load_config(&env);
        require_eligible(&env, &cfg, market_id);

        let obs = NovaClient::new(&env, &cfg.nova).observe(&market_id);
        let collateral = read_collateral(&env, &user, market_id);
        let debt = read_debt(&env, &user, market_id);
        let limit = borrow_limit(&env, collateral, obs.price, cfg.ltv_bps);
        if debt + amount > limit {
            panic_with_error!(&env, Error::BorrowLimitExceeded);
        }

        let loan = token::Client::new(&env, &cfg.loan_token);
        if loan.balance(&env.current_contract_address()) < amount {
            panic_with_error!(&env, Error::InsufficientLiquidity);
        }
        set_persistent(&env, &DataKey::Debt(user.clone(), market_id), &(debt + amount));
        loan.transfer(&env.current_contract_address(), &user, &amount);
    }

    /// Repay debt. Allowed in every market state, so users can always reduce risk.
    /// Returns the amount actually repaid (capped at the outstanding debt).
    pub fn repay(env: Env, user: Address, market_id: u64, amount: i128) -> i128 {
        user.require_auth();
        require_positive(&env, amount);
        let cfg = load_config(&env);
        let debt = read_debt(&env, &user, market_id);
        let repaid = amount.min(debt);
        if repaid > 0 {
            token::Client::new(&env, &cfg.loan_token).transfer(
                &user,
                &env.current_contract_address(),
                &repaid,
            );
            set_persistent(
                &env,
                &DataKey::Debt(user.clone(), market_id),
                &(debt - repaid),
            );
        }
        repaid
    }

    /// Release collateral. Refuses if the remaining collateral would not cover the debt.
    /// NOVA's SBA transfer is gated by `spot_trading`, so this also fails while the market is Degraded.
    pub fn withdraw_collateral(env: Env, user: Address, market_id: u64, amount: i128) {
        user.require_auth();
        require_positive(&env, amount);
        let cfg = load_config(&env);
        let collateral = read_collateral(&env, &user, market_id);
        if collateral < amount {
            panic_with_error!(&env, Error::InsufficientCollateral);
        }
        let remaining = collateral - amount;
        let debt = read_debt(&env, &user, market_id);
        if debt > 0 {
            let obs = NovaClient::new(&env, &cfg.nova).observe(&market_id);
            if debt > borrow_limit(&env, remaining, obs.price, cfg.ltv_bps) {
                panic_with_error!(&env, Error::BorrowLimitExceeded);
            }
        }

        set_persistent(
            &env,
            &DataKey::Collateral(user.clone(), market_id),
            &remaining,
        );
        let vault = env.current_contract_address();
        NovaClient::new(&env, &cfg.nova).transfer(&market_id, &vault, &user, &amount);
    }

    /// Liquidate an unhealthy position: debt above the borrow limit at the current oracle price.
    ///
    /// The liquidator repays up to `CLOSE_FACTOR_BPS` of the debt in loan token and receives
    /// the matching collateral plus `liquidation_bonus_bps`.
    ///
    /// Seized collateral moves through NOVA's `settle_transfer`, which works in Active and
    /// Degraded. It refuses Paused, because a dead oracle leaves no valid price. Deposits and
    /// new borrows still require `is_eligible`, so a Degraded market gets no new risk.
    /// Returns (repaid, seized).
    pub fn liquidate(
        env: Env,
        liquidator: Address,
        borrower: Address,
        market_id: u64,
        repay_amount: i128,
    ) -> (i128, i128) {
        liquidator.require_auth();
        require_positive(&env, repay_amount);
        let cfg = load_config(&env);
        match NovaClient::new(&env, &cfg.nova).state(&market_id) {
            MarketState::Active | MarketState::Degraded => {}
            _ => panic_with_error!(&env, Error::MarketNotEligible),
        }

        let obs = NovaClient::new(&env, &cfg.nova).observe(&market_id);
        if obs.price <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }
        let collateral = read_collateral(&env, &borrower, market_id);
        let debt = read_debt(&env, &borrower, market_id);
        if debt <= borrow_limit(&env, collateral, obs.price, cfg.ltv_bps) {
            panic_with_error!(&env, Error::NotLiquidatable);
        }

        // Round the close-factor cap up so a dust debt can still be closed.
        let max_repay = (debt * CLOSE_FACTOR_BPS + BPS - 1) / BPS;
        let repaid = repay_amount.min(max_repay).min(debt);

        // Seize at oracle price plus the bonus, capped at the borrower's collateral.
        let seized = (repaid * PRICE_SCALE / obs.price
            * (BPS + cfg.liquidation_bonus_bps as i128)
            / BPS)
            .min(collateral);

        token::Client::new(&env, &cfg.loan_token).transfer(
            &liquidator,
            &env.current_contract_address(),
            &repaid,
        );
        set_persistent(
            &env,
            &DataKey::Debt(borrower.clone(), market_id),
            &(debt - repaid),
        );
        set_persistent(
            &env,
            &DataKey::Collateral(borrower.clone(), market_id),
            &(collateral - seized),
        );
        let vault = env.current_contract_address();
        NovaClient::new(&env, &cfg.nova).settle_transfer(&market_id, &vault, &vault, &liquidator, &seized);

        (repaid, seized)
    }

    pub fn position(env: Env, user: Address, market_id: u64) -> Position {
        Position {
            collateral: read_collateral(&env, &user, market_id),
            debt: read_debt(&env, &user, market_id),
        }
    }

    pub fn supplied(env: Env, lender: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Supplied(lender))
            .unwrap_or(0)
    }

    /// Loan-token cash held by the pool.
    pub fn cash(env: Env) -> i128 {
        let cfg = load_config(&env);
        token::Client::new(&env, &cfg.loan_token).balance(&env.current_contract_address())
    }
}

fn load_config(env: &Env) -> Config {
    env.storage()
        .instance()
        .get(&DataKey::Config)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

fn require_positive(env: &Env, amount: i128) {
    if amount <= 0 {
        panic_with_error!(env, Error::InvalidParams);
    }
}

fn require_eligible(env: &Env, cfg: &Config, market_id: u64) {
    if !NovaClient::new(env, &cfg.nova).is_eligible(&market_id) {
        panic_with_error!(env, Error::MarketNotEligible);
    }
}

fn read_collateral(env: &Env, user: &Address, market_id: u64) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Collateral(user.clone(), market_id))
        .unwrap_or(0)
}

fn read_debt(env: &Env, user: &Address, market_id: u64) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Debt(user.clone(), market_id))
        .unwrap_or(0)
}

fn borrow_limit(env: &Env, collateral: i128, price: i128, ltv_bps: u32) -> i128 {
    let value = collateral
        .checked_mul(price)
        .expect("value overflow")
        / PRICE_SCALE;
    value
        .checked_mul(ltv_bps as i128)
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidParams))
        / BPS
}

fn set_persistent(env: &Env, key: &DataKey, value: &i128) {
    env.storage().persistent().set(key, value);
    env.storage()
        .persistent()
        .extend_ttl(key, TTL_THRESHOLD, TTL_EXTEND);
}
