extern crate std;

use super::*;
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
};

const START: u64 = 1_000;
const MAX_AGE: u64 = 86_400;
const SUPPLY: i128 = 1_000_000;
const LOCK: u64 = 30 * 86_400;

#[contract]
pub struct MockOracle;

#[contractimpl]
impl MockOracle {
    pub fn set_price(env: Env, price: i128, timestamp: u64) {
        env.storage()
            .instance()
            .set(&symbol_short!("PX"), &PriceData { price, timestamp });
    }

    pub fn clear(env: Env) {
        env.storage().instance().remove(&symbol_short!("PX"));
    }

    pub fn lastprice(env: Env, _asset: Asset) -> Option<PriceData> {
        env.storage().instance().get(&symbol_short!("PX"))
    }
}

#[contract]
pub struct MockLiquidity;

#[contractimpl]
impl MockLiquidity {
    pub fn set(env: Env, bid: i128, ask: i128, depth: i128) {
        env.storage()
            .instance()
            .set(&symbol_short!("SNAP"), &LiquiditySnapshot { bid, ask, depth });
    }

    pub fn snapshot(env: Env, _market_id: u64) -> LiquiditySnapshot {
        env.storage()
            .instance()
            .get(&symbol_short!("SNAP"))
            .unwrap()
    }
}

struct Fixture {
    env: Env,
    nova_id: Address,
    oracle_id: Address,
    liq_id: Address,
    bond_id: Address,
    creator: Address,
}

impl Fixture {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(START);

        let nova_id = env.register(Nova, ());
        let oracle_id = env.register(MockOracle, ());
        let liq_id = env.register(MockLiquidity, ());

        let token_admin = Address::generate(&env);
        let bond_id = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let creator = Address::generate(&env);
        StellarAssetClient::new(&env, &bond_id).mint(&creator, &(MIN_BOND * 10));

        let fixture = Fixture {
            env,
            nova_id,
            oracle_id,
            liq_id,
            bond_id,
            creator,
        };
        fixture.oracle().set_price(&1_000_0000000, &START);
        fixture.liquidity().set(&9_990, &10_000, &50_000);
        let settler = Address::generate(&fixture.env);
        fixture.nova().initialize(
            &soroban_sdk::vec![&fixture.env, fixture.oracle_id.clone()],
            &soroban_sdk::vec![&fixture.env, fixture.liq_id.clone()],
            &soroban_sdk::vec![&fixture.env, settler],
        );
        fixture
    }

    fn nova(&self) -> NovaClient<'_> {
        NovaClient::new(&self.env, &self.nova_id)
    }

    fn oracle(&self) -> MockOracleClient<'_> {
        MockOracleClient::new(&self.env, &self.oracle_id)
    }

    fn liquidity(&self) -> MockLiquidityClient<'_> {
        MockLiquidityClient::new(&self.env, &self.liq_id)
    }

    fn liquidity_for(&self, adapter: &Address) -> MockLiquidityClient<'_> {
        MockLiquidityClient::new(&self.env, adapter)
    }

    fn bond(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.bond_id)
    }

    fn constitution(&self) -> Constitution {
        Constitution {
            max_spread_bps: 350,
            max_oracle_age_secs: MAX_AGE,
            min_depth: 5_000,
            target_signers: 3,
        }
    }

    fn create(&self) -> u64 {
        self.nova().create_market(
            &self.creator,
            &symbol_short!("LITHIUM"),
            &self.oracle_id,
            &Asset::Other(symbol_short!("LITHIUM")),
            &self.liq_id,
            &self.constitution(),
            &self.bond_id,
            &MIN_BOND,
            &LOCK,
            &SUPPLY,
        )
    }

    /// Drive a market from Discovery to Active with fresh data.
    fn activate(&self, id: u64) {
        for _ in 0..3 {
            self.nova().signal_intent(&id, &Address::generate(&self.env));
        }
        self.refresh_feeds();
        self.nova().checkpoint(&id);
    }

    fn refresh_feeds(&self) {
        self.oracle()
            .set_price(&1_000_0000000, &self.env.ledger().timestamp());
    }

    fn advance(&self, secs: u64) {
        self.env
            .ledger()
            .set_timestamp(self.env.ledger().timestamp() + secs);
    }
}

#[test]
fn create_locks_bond_and_mints_supply_in_discovery() {
    let f = Fixture::new();
    let before = f.bond().balance(&f.creator);
    let id = f.create();

    assert_eq!(f.bond().balance(&f.creator), before - MIN_BOND);
    assert_eq!(f.bond().balance(&f.nova_id), MIN_BOND);
    assert_eq!(f.nova().get_market(&id).state, MarketState::Discovery);
    assert_eq!(f.nova().balance(&id, &f.creator), SUPPLY);
    assert!(!f.nova().is_eligible(&id));
    assert!(!f.nova().caps(&id).spot_trading);
}

#[test]
fn initialize_is_one_shot_and_rejects_empty_list() {
    let f = Fixture::new();
    // Fixture already initialized both allowlists.
    assert!(f
        .nova()
        .try_initialize(
            &soroban_sdk::vec![&f.env, f.oracle_id.clone()],
            &soroban_sdk::vec![&f.env, f.liq_id.clone()],
            &soroban_sdk::vec![&f.env, f.creator.clone()],
        )
        .is_err());
    assert!(f.nova().is_approved_oracle(&f.oracle_id));
    assert!(f.nova().is_approved_liquidity(&f.liq_id));

    // A fresh instance cannot be initialized with an empty list in either slot.
    let fresh_id = f.env.register(Nova, ());
    let fresh = NovaClient::new(&f.env, &fresh_id);
    let one = soroban_sdk::vec![&f.env, f.oracle_id.clone()];
    let settlers = soroban_sdk::vec![&f.env, f.creator.clone()];
    assert!(fresh
        .try_initialize(
            &soroban_sdk::vec![&f.env],
            &soroban_sdk::vec![&f.env, f.liq_id.clone()],
            &settlers,
        )
        .is_err());
    assert!(fresh
        .try_initialize(&one, &soroban_sdk::vec![&f.env], &settlers)
        .is_err());
    assert!(fresh
        .try_initialize(&one, &soroban_sdk::vec![&f.env, f.liq_id.clone()], &soroban_sdk::vec![&f.env])
        .is_err());
}

#[test]
fn create_rejects_unapproved_liquidity_adapter() {
    let f = Fixture::new();
    let rogue_liq = f.env.register(MockLiquidity, ());
    f.liquidity_for(&rogue_liq).set(&9_990, &10_000, &50_000);
    let result = f.nova().try_create_market(
        &f.creator,
        &symbol_short!("LITHIUM"),
        &f.oracle_id,
        &Asset::Other(symbol_short!("LITHIUM")),
        &rogue_liq,
        &f.constitution(),
        &f.bond_id,
        &MIN_BOND,
        &LOCK,
        &SUPPLY,
    );
    assert!(result.is_err());
    assert!(!f.nova().is_approved_liquidity(&rogue_liq));
}

#[test]
fn create_rejects_unapproved_oracle() {
    let f = Fixture::new();
    let rogue_oracle = f.env.register(MockOracle, ());
    let result = f.nova().try_create_market(
        &f.creator,
        &symbol_short!("LITHIUM"),
        &rogue_oracle,
        &Asset::Other(symbol_short!("LITHIUM")),
        &f.liq_id,
        &f.constitution(),
        &f.bond_id,
        &MIN_BOND,
        &LOCK,
        &SUPPLY,
    );
    assert!(result.is_err());
    assert!(!f.nova().is_approved_oracle(&rogue_oracle));
}

#[test]
fn create_rejects_bond_below_minimum() {
    let f = Fixture::new();
    let result = f.nova().try_create_market(
        &f.creator,
        &symbol_short!("LITHIUM"),
        &f.oracle_id,
        &Asset::Other(symbol_short!("LITHIUM")),
        &f.liq_id,
        &f.constitution(),
        &f.bond_id,
        &(MIN_BOND - 1),
        &LOCK,
        &SUPPLY,
    );
    assert!(result.is_err());
}

#[test]
fn intent_is_unique_and_reaching_target_moves_to_ready() {
    let f = Fixture::new();
    let id = f.create();
    let (a, b, c) = (
        Address::generate(&f.env),
        Address::generate(&f.env),
        Address::generate(&f.env),
    );

    assert_eq!(f.nova().signal_intent(&id, &a), 1);
    assert!(f.nova().try_signal_intent(&id, &a).is_err());
    assert_eq!(f.nova().signal_intent(&id, &b), 2);
    assert_eq!(f.nova().get_market(&id).state, MarketState::Discovery);

    assert_eq!(f.nova().signal_intent(&id, &c), 3);
    assert_eq!(f.nova().get_market(&id).state, MarketState::Ready);

    // Ready is pre-active: no trading until a checkpoint confirms health.
    assert!(!f.nova().caps(&id).spot_trading);
    f.refresh_feeds();
    assert_eq!(f.nova().checkpoint(&id), MarketState::Active);
    assert!(f.nova().is_eligible(&id));
}

#[test]
fn healthy_market_allows_sba_transfers() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    let alice = Address::generate(&f.env);
    f.nova().transfer(&id, &f.creator, &alice, &100);
    assert_eq!(f.nova().balance(&id, &alice), 100);
    assert_eq!(f.nova().balance(&id, &f.creator), SUPPLY - 100);
}

#[test]
fn transfers_are_blocked_before_activation() {
    let f = Fixture::new();
    let id = f.create();
    let alice = Address::generate(&f.env);
    assert!(f.nova().try_transfer(&id, &f.creator, &alice, &1).is_err());
}

#[test]
fn stale_oracle_pauses_and_blocks_everything() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    // Oracle last updated more than MAX_AGE ago.
    f.oracle().set_price(&1_000_0000000, &START);
    f.advance(MAX_AGE + 1);
    f.liquidity().set(&9_990, &10_000, &50_000);
    assert_eq!(f.nova().checkpoint(&id), MarketState::Paused);

    let caps = f.nova().caps(&id);
    assert!(!caps.reference_pricing && !caps.spot_trading && !caps.collateral_eligible);
    assert!(!f.nova().is_eligible(&id));
    let alice = Address::generate(&f.env);
    assert!(f.nova().try_transfer(&id, &f.creator, &alice, &1).is_err());
}

#[test]
fn dead_oracle_pauses_the_market() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    f.oracle().clear();
    assert_eq!(f.nova().observe(&id).health, MarketState::Paused);
    assert_eq!(f.nova().checkpoint(&id), MarketState::Paused);
}

#[test]
fn wide_spread_degrades_but_keeps_reference_pricing() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    // ~540 bps spread, above the 350 bps limit.
    f.liquidity().set(&9_000, &9_500, &50_000);
    f.refresh_feeds();
    assert_eq!(f.nova().checkpoint(&id), MarketState::Degraded);

    let caps = f.nova().caps(&id);
    assert!(caps.reference_pricing);
    assert!(!caps.spot_trading);
    assert!(!caps.collateral_eligible);
    assert!(!f.nova().is_eligible(&id));

    // Recovery is automatic on the next checkpoint.
    f.liquidity().set(&9_990, &10_000, &50_000);
    assert_eq!(f.nova().checkpoint(&id), MarketState::Active);
}

#[test]
fn thin_depth_degrades() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    f.liquidity().set(&9_990, &10_000, &100);
    f.refresh_feeds();
    let obs = f.nova().observe(&id);
    assert_eq!(obs.health, MarketState::Degraded);
    assert_eq!(obs.depth, 100);
    assert_eq!(f.nova().checkpoint(&id), MarketState::Degraded);
}

#[test]
fn stale_checkpoint_halts_until_someone_checkpoints() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    f.advance(MAX_AGE + 1);
    // No checkpoint was written, so the stored state is still Active,
    // but the effective state is Paused.
    assert_eq!(f.nova().get_market(&id).state, MarketState::Active);
    assert_eq!(f.nova().state(&id), MarketState::Paused);
    assert!(!f.nova().is_eligible(&id));

    // Anyone can heal it by checkpointing with fresh data.
    f.refresh_feeds();
    assert_eq!(f.nova().checkpoint(&id), MarketState::Active);
    assert!(f.nova().is_eligible(&id));
}

#[test]
fn bond_stays_locked_until_unlock() {
    let f = Fixture::new();
    let id = f.create();
    assert!(f.nova().try_withdraw_bond(&id).is_err());

    f.advance(LOCK);
    f.nova().withdraw_bond(&id);
    assert_eq!(f.bond().balance(&f.nova_id), 0);
    assert!(f.nova().try_withdraw_bond(&id).is_err());
}

#[test]
fn observe_does_not_write_state() {
    let f = Fixture::new();
    let id = f.create();
    f.activate(id);

    f.oracle().set_price(&1_000_0000000, &START);
    f.advance(MAX_AGE + 1);
    assert_eq!(f.nova().observe(&id).health, MarketState::Paused);
    assert_eq!(f.nova().get_market(&id).state, MarketState::Active);
}
