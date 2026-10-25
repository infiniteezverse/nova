#![no_std]
//! NOVA: a State-Bound Asset (SBA) registry on Stellar.
//!
//! A market mints an asset whose utility is gated by on-chain health:
//! - `create_market` locks a creator bond (SEP-41) and mints the supply to the creator.
//! - `signal_intent` records unique signers; reaching the target moves the market to `Ready`.
//! - `observe` reads the SEP-40 oracle and a liquidity source and reports health (no writes).
//! - `checkpoint` (permissionless) writes the evaluated health into the market state.
//! - `is_eligible` is the composable shield for lending/vaults. It is a pure read.
//! - `transfer` of the SBA token is gated by the effective `spot_trading` capability.
//!
//! There is no admin key anywhere in this contract.

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype,
    panic_with_error, token, Address, Env, Symbol, Vec,
};

/// Minimum creator bond: 1,000 units at 7 decimals (Stellar's standard).
pub const MIN_BOND: i128 = 1_000 * 10_000_000;
const BPS: i128 = 10_000;
/// Spread value used when the liquidity source has no usable two-sided quote.
const UNPRICED_SPREAD_BPS: i128 = u32::MAX as i128;

const LEDGER_DAY: u32 = 17_280;
const TTL_THRESHOLD: u32 = LEDGER_DAY * 7;
const TTL_EXTEND: u32 = LEDGER_DAY * 30;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotFound = 1,
    InvalidParams = 2,
    BondTooSmall = 3,
    WrongState = 4,
    AlreadySigned = 5,
    BondLocked = 6,
    BondAlreadyReleased = 7,
    InsufficientBalance = 8,
    TransferHalted = 9,
    UnapprovedOracle = 10,
    AlreadyInitialized = 11,
    NotInitialized = 12,
    EmptyAllowlist = 13,
    UnapprovedLiquidity = 14,
    UnapprovedSettler = 15,
    Unauthorized = 16,
}

/// Lifecycle + health states. `Discovery` and `Ready` are pre-activation states.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MarketState {
    Discovery,
    Ready,
    Active,
    Degraded,
    Paused,
}

/// Immutable thresholds written at creation (the market's "constitution").
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Constitution {
    pub max_spread_bps: u32,
    pub max_oracle_age_secs: u64,
    pub min_depth: i128,
    pub target_signers: u32,
}

/// SEP-40 asset identifier.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Asset {
    Stellar(Address),
    Other(Symbol),
}

/// SEP-40 price record.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

/// Two-sided quote plus quote-side depth, supplied by a liquidity source (e.g. a Soroswap adapter).
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LiquiditySnapshot {
    pub bid: i128,
    pub ask: i128,
    pub depth: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Market {
    pub creator: Address,
    pub symbol: Symbol,
    pub oracle: Address,
    pub oracle_asset: Asset,
    pub liquidity: Address,
    pub constitution: Constitution,
    pub bond_token: Address,
    pub bond_amount: i128,
    pub unlock_time: u64,
    pub bond_released: bool,
    pub supply: i128,
    pub signers: u32,
    pub state: MarketState,
    pub last_checkpoint: u64,
}

/// Granular capability bitmask derived from the market state.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Caps {
    pub reference_pricing: bool,
    pub spot_trading: bool,
    pub collateral_eligible: bool,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    pub health: MarketState,
    pub price: i128,
    pub oracle_timestamp: u64,
    pub spread_bps: i128,
    pub depth: i128,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    NextId,
    Oracles,
    Liquidity,
    Settlers,
    Market(u64),
    Balance(u64, Address),
    Signed(u64, Address),
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketCreated {
    #[topic]
    pub market_id: u64,
    pub creator: Address,
    pub symbol: Symbol,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntentSignaled {
    #[topic]
    pub market_id: u64,
    pub signers: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateChanged {
    #[topic]
    pub market_id: u64,
    pub from: MarketState,
    pub to: MarketState,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbaTransfer {
    #[topic]
    pub market_id: u64,
    pub from: Address,
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BondWithdrawn {
    #[topic]
    pub market_id: u64,
    pub amount: i128,
}

/// SEP-40 oracle interface (subset used by NOVA).
#[contractclient(name = "OracleClient")]
pub trait Sep40Oracle {
    fn lastprice(env: Env, asset: Asset) -> Option<PriceData>;
}

/// Liquidity source interface. A Soroswap adapter implements this for a given market.
#[contractclient(name = "LiquidityClient")]
pub trait LiquiditySource {
    fn snapshot(env: Env, market_id: u64) -> LiquiditySnapshot;
}

#[contract]
pub struct Nova;

#[contractimpl]
impl Nova {
    /// Set the oracle, liquidity-adapter, and settler allowlists. Runs once. No function
    /// changes any list afterward, so the protocol has no admin key over what it trusts.
    ///
    /// `settlers` are lending contracts allowed to call `settle_transfer`. Deploy the
    /// lending contract first, then initialize NOVA with its address.
    pub fn initialize(
        env: Env,
        oracles: Vec<Address>,
        liquidity: Vec<Address>,
        settlers: Vec<Address>,
    ) {
        if env.storage().instance().has(&DataKey::Oracles) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        if oracles.is_empty() || liquidity.is_empty() || settlers.is_empty() {
            panic_with_error!(&env, Error::EmptyAllowlist);
        }
        env.storage().instance().set(&DataKey::Oracles, &oracles);
        env.storage().instance().set(&DataKey::Liquidity, &liquidity);
        env.storage().instance().set(&DataKey::Settlers, &settlers);
    }

    pub fn is_approved_oracle(env: Env, oracle: Address) -> bool {
        approved_oracles(&env).contains(&oracle)
    }

    pub fn is_approved_liquidity(env: Env, adapter: Address) -> bool {
        approved_liquidity(&env).contains(&adapter)
    }

    pub fn is_approved_settler(env: Env, settler: Address) -> bool {
        approved_settlers(&env).contains(&settler)
    }

    /// Lock the creator's bond and mint the SBA supply to the creator.
    /// The market starts in `Discovery`. `lock_secs` sets the bond's unlock time.
    #[allow(clippy::too_many_arguments)]
    pub fn create_market(
        env: Env,
        creator: Address,
        symbol: Symbol,
        oracle: Address,
        oracle_asset: Asset,
        liquidity: Address,
        constitution: Constitution,
        bond_token: Address,
        bond_amount: i128,
        lock_secs: u64,
        supply: i128,
    ) -> u64 {
        creator.require_auth();

        if constitution.max_spread_bps == 0
            || constitution.max_spread_bps as i128 > BPS
            || constitution.max_oracle_age_secs == 0
            || constitution.min_depth < 0
            || constitution.target_signers == 0
            || supply <= 0
        {
            panic_with_error!(&env, Error::InvalidParams);
        }
        if bond_amount < MIN_BOND {
            panic_with_error!(&env, Error::BondTooSmall);
        }
        if !approved_oracles(&env).contains(&oracle) {
            panic_with_error!(&env, Error::UnapprovedOracle);
        }
        if !approved_liquidity(&env).contains(&liquidity) {
            panic_with_error!(&env, Error::UnapprovedLiquidity);
        }

        token::Client::new(&env, &bond_token).transfer(
            &creator,
            &env.current_contract_address(),
            &bond_amount,
        );

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let now = env.ledger().timestamp();
        let market = Market {
            creator: creator.clone(),
            symbol: symbol.clone(),
            oracle,
            oracle_asset,
            liquidity,
            constitution,
            bond_token,
            bond_amount,
            unlock_time: now.checked_add(lock_secs).expect("unlock overflow"),
            bond_released: false,
            supply,
            signers: 0,
            state: MarketState::Discovery,
            last_checkpoint: now,
        };
        save_market(&env, id, &market);
        set_balance(&env, id, &creator, supply);

        MarketCreated {
            market_id: id,
            creator,
            symbol,
        }
        .publish(&env);
        id
    }

    /// Record a unique signer. Reaching the target moves the market to `Ready`.
    /// `checkpoint` then promotes it to `Active` once health checks pass.
    pub fn signal_intent(env: Env, market_id: u64, signer: Address) -> u32 {
        signer.require_auth();
        let mut market = load_market(&env, market_id);
        if market.state != MarketState::Discovery {
            panic_with_error!(&env, Error::WrongState);
        }

        let key = DataKey::Signed(market_id, signer.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::AlreadySigned);
        }
        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);

        market.signers += 1;
        if market.signers >= market.constitution.target_signers {
            market.state = MarketState::Ready;
        }
        save_market(&env, market_id, &market);

        IntentSignaled {
            market_id,
            signers: market.signers,
        }
        .publish(&env);
        market.signers
    }

    /// Read-only: evaluate the constitution against live oracle and liquidity data.
    /// Writes nothing. Anyone can call it through simulation.
    pub fn observe(env: Env, market_id: u64) -> Observation {
        let market = load_market(&env, market_id);
        evaluate(&env, market_id, &market)
    }

    /// Permissionless: evaluate health and write the resulting state.
    /// Returns the state after the checkpoint.
    pub fn checkpoint(env: Env, market_id: u64) -> MarketState {
        let mut market = load_market(&env, market_id);
        let observation = evaluate(&env, market_id, &market);
        let next = next_state(&market, observation.health);

        if next != market.state {
            StateChanged {
                market_id,
                from: market.state,
                to: next,
            }
            .publish(&env);
        }
        market.state = next;
        market.last_checkpoint = env.ledger().timestamp();
        save_market(&env, market_id, &market);
        next
    }

    /// Composable shield. True only when the effective state is `Active`.
    /// A stale checkpoint counts as `Paused`, so this is never trusted past its window.
    pub fn is_eligible(env: Env, market_id: u64) -> bool {
        let market = load_market(&env, market_id);
        effective_state(&env, &market) == MarketState::Active
    }

    /// Effective state, accounting for a stale checkpoint.
    pub fn state(env: Env, market_id: u64) -> MarketState {
        let market = load_market(&env, market_id);
        effective_state(&env, &market)
    }

    pub fn caps(env: Env, market_id: u64) -> Caps {
        let market = load_market(&env, market_id);
        caps_for(effective_state(&env, &market))
    }

    /// Settlement transfer for allowlisted lending contracts (`settlers`).
    ///
    /// Two limits apply:
    /// - Only `settler`'s own balance can move. `from` must equal `settler`, so a lending
    ///   contract can release only the collateral it holds, never a user's balance.
    /// - It works in Active and Degraded. It does not work in Paused, Discovery, or Ready.
    ///   Degraded halts open trading, but a bounded liquidation can still settle.
    pub fn settle_transfer(
        env: Env,
        market_id: u64,
        settler: Address,
        from: Address,
        to: Address,
        amount: i128,
    ) {
        settler.require_auth();
        if !approved_settlers(&env).contains(&settler) {
            panic_with_error!(&env, Error::UnapprovedSettler);
        }
        if from != settler {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidParams);
        }
        let market = load_market(&env, market_id);
        match effective_state(&env, &market) {
            MarketState::Active | MarketState::Degraded => {}
            _ => panic_with_error!(&env, Error::TransferHalted),
        }
        let from_balance = get_balance(&env, market_id, &from);
        if from_balance < amount {
            panic_with_error!(&env, Error::InsufficientBalance);
        }
        set_balance(&env, market_id, &from, from_balance - amount);
        set_balance(&env, market_id, &to, get_balance(&env, market_id, &to) + amount);
        SbaTransfer {
            market_id,
            from,
            to,
            amount,
        }
        .publish(&env);
    }

    pub fn get_market(env: Env, market_id: u64) -> Market {
        load_market(&env, market_id)
    }

    pub fn balance(env: Env, market_id: u64, who: Address) -> i128 {
        get_balance(&env, market_id, &who)
    }

    /// SBA token transfer. Halted unless the effective `spot_trading` capability is on.
    pub fn transfer(env: Env, market_id: u64, from: Address, to: Address, amount: i128) {
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidParams);
        }
        let market = load_market(&env, market_id);
        if !caps_for(effective_state(&env, &market)).spot_trading {
            panic_with_error!(&env, Error::TransferHalted);
        }
        let from_balance = get_balance(&env, market_id, &from);
        if from_balance < amount {
            panic_with_error!(&env, Error::InsufficientBalance);
        }
        set_balance(&env, market_id, &from, from_balance - amount);
        set_balance(
            &env,
            market_id,
            &to,
            get_balance(&env, market_id, &to) + amount,
        );
        SbaTransfer {
            market_id,
            from,
            to,
            amount,
        }
        .publish(&env);
    }

    /// Creator reclaims the bond after the lock expires. Reverts before unlock.
    pub fn withdraw_bond(env: Env, market_id: u64) {
        let mut market = load_market(&env, market_id);
        market.creator.require_auth();
        if market.bond_released {
            panic_with_error!(&env, Error::BondAlreadyReleased);
        }
        if env.ledger().timestamp() < market.unlock_time {
            panic_with_error!(&env, Error::BondLocked);
        }
        market.bond_released = true;
        save_market(&env, market_id, &market);

        token::Client::new(&env, &market.bond_token).transfer(
            &env.current_contract_address(),
            &market.creator,
            &market.bond_amount,
        );
        BondWithdrawn {
            market_id,
            amount: market.bond_amount,
        }
        .publish(&env);
    }
}

fn approved_oracles(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Oracles)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

fn approved_settlers(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Settlers)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

fn approved_liquidity(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Liquidity)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

fn load_market(env: &Env, market_id: u64) -> Market {
    env.storage()
        .persistent()
        .get(&DataKey::Market(market_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::NotFound))
}

fn save_market(env: &Env, market_id: u64, market: &Market) {
    let key = DataKey::Market(market_id);
    env.storage().persistent().set(&key, market);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}

fn get_balance(env: &Env, market_id: u64, who: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Balance(market_id, who.clone()))
        .unwrap_or(0)
}

fn set_balance(env: &Env, market_id: u64, who: &Address, amount: i128) {
    let key = DataKey::Balance(market_id, who.clone());
    env.storage().persistent().set(&key, &amount);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}

/// Stored state, overridden to `Paused` when the last checkpoint is older than the oracle-age limit.
fn effective_state(env: &Env, market: &Market) -> MarketState {
    let age = env
        .ledger()
        .timestamp()
        .saturating_sub(market.last_checkpoint);
    if age > market.constitution.max_oracle_age_secs {
        MarketState::Paused
    } else {
        market.state
    }
}

fn caps_for(state: MarketState) -> Caps {
    match state {
        MarketState::Active => Caps {
            reference_pricing: true,
            spot_trading: true,
            collateral_eligible: true,
        },
        MarketState::Discovery | MarketState::Ready | MarketState::Degraded => Caps {
            reference_pricing: true,
            spot_trading: false,
            collateral_eligible: false,
        },
        MarketState::Paused => Caps {
            reference_pricing: false,
            spot_trading: false,
            collateral_eligible: false,
        },
    }
}

/// Rules 01–04. Oracle failure or staleness pauses. Spread or depth breaches degrade.
fn evaluate(env: &Env, market_id: u64, market: &Market) -> Observation {
    let now = env.ledger().timestamp();
    let max_age = market.constitution.max_oracle_age_secs;

    let feed = OracleClient::new(env, &market.oracle).lastprice(&market.oracle_asset);
    let paused = |price, oracle_timestamp| Observation {
        health: MarketState::Paused,
        price,
        oracle_timestamp,
        spread_bps: 0,
        depth: 0,
    };
    let (price, oracle_timestamp) = match feed {
        Some(p) if p.price > 0 && p.timestamp <= now && now - p.timestamp <= max_age => {
            (p.price, p.timestamp)
        }
        Some(p) => return paused(p.price, p.timestamp),
        None => return paused(0, 0),
    };

    let snap = LiquidityClient::new(env, &market.liquidity).snapshot(&market_id);
    let spread_bps = if snap.bid <= 0 || snap.ask < snap.bid {
        UNPRICED_SPREAD_BPS
    } else {
        let mid = (snap.ask + snap.bid) / 2;
        (snap.ask - snap.bid)
            .checked_mul(BPS)
            .expect("spread overflow")
            / mid
    };

    let breached = spread_bps > market.constitution.max_spread_bps as i128
        || snap.depth < market.constitution.min_depth;
    Observation {
        health: if breached {
            MarketState::Degraded
        } else {
            MarketState::Active
        },
        price,
        oracle_timestamp,
        spread_bps,
        depth: snap.depth,
    }
}

/// Map a fresh health reading onto the lifecycle. Pre-target markets stay in `Discovery`.
fn next_state(market: &Market, health: MarketState) -> MarketState {
    if health == MarketState::Paused {
        MarketState::Paused
    } else if market.signers < market.constitution.target_signers {
        MarketState::Discovery
    } else {
        health
    }
}

#[cfg(test)]
mod test;
