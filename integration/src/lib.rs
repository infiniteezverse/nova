//! Integration test: NOVA + the real Soroswap adapter WASM + real Soroswap
//! factory/pair/router WASMs (built from github.com/soroswap/core).
//!
//! Every contract here is a compiled WASM, not a Rust mock. The only Rust mock is
//! the SEP-40 oracle, because no SEP-40 oracle WASM is checked in.
//!
//! The WASMs in `wasm/` must be rebuilt before running this test. NOVA and the
//! adapter come from `stellar contract build` in their own crates. The Soroswap
//! WASMs come from github.com/soroswap/core (see the README).

#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    testutils::{Address as _, Ledger},
    token, token::StellarAssetClient,
    vec, Address, Env, Symbol,
};

mod nova {
    soroban_sdk::contractimport!(file = "wasm/nova_sba.wasm");
}
mod adapter {
    soroban_sdk::contractimport!(file = "wasm/soroswap_adapter.wasm");
}
mod router {
    soroban_sdk::contractimport!(file = "wasm/soroswap_router.wasm");
}
mod factory {
    soroban_sdk::contractimport!(file = "wasm/soroswap_factory.wasm");
}
mod pair {
    soroban_sdk::contractimport!(file = "wasm/soroswap_pair.wasm");
}
mod vault {
    soroban_sdk::contractimport!(file = "wasm/nova_vault.wasm");
}

const START: u64 = 1_000;
const MIN_BOND: i128 = 10_000_000_000; // matches nova::MIN_BOND (1,000 units at 7 decimals)
const LOCK: u64 = 30 * 86_400;
const SUPPLY: i128 = 1_000_000;
const POOL_SIDE: i128 = 1_000_000;
const MIN_DEPTH: i128 = 500_000;

// SEP-40 oracle mock with the same XDR shape as NOVA's oracle interface.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OracleAsset {
    Stellar(Address),
    Other(Symbol),
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contract]
pub struct MockOracle;

#[contractimpl]
impl MockOracle {
    pub fn set_price(env: Env, price: i128, timestamp: u64) {
        env.storage()
            .instance()
            .set(&symbol_short!("PX"), &PriceData { price, timestamp });
    }

    pub fn lastprice(env: Env, _asset: OracleAsset) -> Option<PriceData> {
        env.storage().instance().get(&symbol_short!("PX"))
    }
}

/// Holds the deployed addresses. Clients are built on demand so they borrow `env` locally.
struct World {
    env: Env,
    nova_id: Address,
    adapter_id: Address,
    router_id: Address,
    oracle_id: Address,
    base: Address,
    quote: Address,
    bond: Address,
    creator: Address,
    trader: Address,
    lp: Address,
    vault_id: Address,
}

impl World {
    fn nova(&self) -> nova::Client<'_> {
        nova::Client::new(&self.env, &self.nova_id)
    }

    fn adapter(&self) -> adapter::Client<'_> {
        adapter::Client::new(&self.env, &self.adapter_id)
    }

    fn router(&self) -> router::Client<'_> {
        router::Client::new(&self.env, &self.router_id)
    }

    fn oracle(&self) -> MockOracleClient<'_> {
        MockOracleClient::new(&self.env, &self.oracle_id)
    }

    fn constitution(&self) -> nova::Constitution {
        nova::Constitution {
            max_spread_bps: 350,
            max_oracle_age_secs: 86_400,
            min_depth: MIN_DEPTH,
            target_signers: 3,
        }
    }

    fn create_market(&self) -> u64 {
        self.nova().create_market(
            &self.creator,
            &symbol_short!("LITH"),
            &self.oracle_id,
            &nova::Asset::Stellar(self.base.clone()),
            &self.adapter_id,
            &self.constitution(),
            &self.bond,
            &MIN_BOND,
            &LOCK,
            &SUPPLY,
        )
    }

    fn refresh_oracle(&self) {
        self.oracle()
            .set_price(&10_000_000, &self.env.ledger().timestamp());
    }
}

fn token(env: &Env, admin: &Address) -> Address {
    env.register_stellar_asset_contract_v2(admin.clone()).address()
}

fn setup() -> World {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(START);

    let admin = Address::generate(&env);
    let base = token(&env, &admin);
    let quote = token(&env, &admin);
    let bond = token(&env, &admin);

    let creator = Address::generate(&env);
    let trader = Address::generate(&env);
    let lp = Address::generate(&env);
    StellarAssetClient::new(&env, &bond).mint(&creator, &(MIN_BOND * 10));
    StellarAssetClient::new(&env, &base).mint(&lp, &(POOL_SIDE * 10));
    StellarAssetClient::new(&env, &quote).mint(&lp, &(POOL_SIDE * 10));
    StellarAssetClient::new(&env, &base).mint(&trader, &(POOL_SIDE * 10));

    // Soroswap factory (needs the pair WASM hash), then the router pointing at the factory.
    let pair_hash = env.deployer().upload_contract_wasm(pair::WASM);
    let factory_id = env.register(factory::WASM, ());
    factory::Client::new(&env, &factory_id).initialize(&admin, &pair_hash);
    let router_id = env.register(router::WASM, ());
    router::Client::new(&env, &router_id).initialize(&factory_id);

    // Seed the pool. The router creates the pair on first liquidity.
    router::Client::new(&env, &router_id).add_liquidity(
        &base,
        &quote,
        &POOL_SIDE,
        &POOL_SIDE,
        &0,
        &0,
        &lp,
        &(START + 10_000),
    );

    // Real adapter, bound to that pool.
    let adapter_id = env.register(adapter::WASM, ());
    adapter::Client::new(&env, &adapter_id).initialize(&router_id, &base, &quote, &1_000);

    // Oracle and NOVA, with both allowlists set to the deployed addresses.
    let oracle_id = env.register(MockOracle, ());
    MockOracleClient::new(&env, &oracle_id).set_price(&10_000_000, &START);

    // Register NOVA and the vault first. NOVA's settler allowlist names the vault,
    // and the vault's initialize takes NOVA's address. Registering both before any
    // initialize call breaks that cycle.
    let nova_id = env.register(nova::WASM, ());
    let vault_id = env.register(vault::WASM, ());
    nova::Client::new(&env, &nova_id).initialize(
        &vec![&env, oracle_id.clone()],
        &vec![&env, adapter_id.clone()],
        &vec![&env, vault_id.clone()],
    );

    World {
        env,
        nova_id,
        adapter_id,
        router_id,
        oracle_id,
        base,
        quote,
        bond,
        creator,
        trader,
        lp,
        vault_id,
    }
}

#[test]
fn real_soroswap_pool_drives_nova_constitution() {
    let w = setup();
    let id = w.create_market();

    // The real router quotes through the adapter: a two-sided market with real depth.
    let before = w.nova().observe(&id);
    assert!(before.spread_bps > 0 && before.spread_bps < 350, "spread {}", before.spread_bps);
    assert_eq!(before.depth, POOL_SIDE, "depth should be the pool's quote reserve");

    // Three unique signers move the market to Ready. A checkpoint then activates it.
    for _ in 0..3 {
        w.nova().signal_intent(&id, &Address::generate(&w.env));
    }
    assert_eq!(w.nova().get_market(&id).state, nova::MarketState::Ready);
    w.refresh_oracle();
    assert_eq!(w.nova().checkpoint(&id), nova::MarketState::Active);
    assert!(w.nova().is_eligible(&id));

    // Active market: SBA transfers work.
    let alice = Address::generate(&w.env);
    w.nova().transfer(&id, &w.creator, &alice, &100);
    assert_eq!(w.nova().balance(&id, &alice), 100);

    // A real swap drains the quote side. Depth falls below MIN_DEPTH.
    w.router().swap_exact_tokens_for_tokens(
        &(POOL_SIDE * 3 / 2),
        &0,
        &vec![&w.env, w.base.clone(), w.quote.clone()],
        &w.trader,
        &(START + 10_000),
    );

    let after = w.nova().observe(&id);
    assert!(after.depth < MIN_DEPTH, "depth after swap {}", after.depth);
    assert_eq!(after.health, nova::MarketState::Degraded);
    assert_eq!(w.nova().checkpoint(&id), nova::MarketState::Degraded);

    let caps = w.nova().caps(&id);
    assert!(caps.reference_pricing);
    assert!(!caps.spot_trading);
    assert!(!caps.collateral_eligible);
    assert!(!w.nova().is_eligible(&id));
    assert!(w.nova().try_transfer(&id, &w.creator, &alice, &1).is_err());
}

// ---- Lending vault tests ----

const LTV_BPS: u32 = 5_000;
const BONUS_BPS: u32 = 500;
const LENDER_LIQUIDITY: i128 = 500_000;
const COLLATERAL: i128 = 100_000;

/// Activates market `id`: three signers, a fresh oracle price, then a checkpoint.
fn activate(w: &World, id: u64) {
    for _ in 0..3 {
        w.nova().signal_intent(&id, &Address::generate(&w.env));
    }
    w.refresh_oracle();
    assert_eq!(w.nova().checkpoint(&id), nova::MarketState::Active);
}

/// Real swap that drains quote-side depth below MIN_DEPTH, then a checkpoint.
fn degrade(w: &World, id: u64) {
    w.router().swap_exact_tokens_for_tokens(
        &(POOL_SIDE * 3 / 2),
        &0,
        &vec![&w.env, w.base.clone(), w.quote.clone()],
        &w.trader,
        &(START + 10_000),
    );
    assert_eq!(w.nova().checkpoint(&id), nova::MarketState::Degraded);
}

/// Real liquidity added by the LP. This pushes quote depth back above MIN_DEPTH.
fn restore_liquidity(w: &World) {
    w.router().add_liquidity(
        &w.base,
        &w.quote,
        &2_000_000,
        &2_000_000,
        &0,
        &0,
        &w.lp,
        &(START + 10_000),
    );
}

/// Deploys the vault against NOVA and funds it with lender liquidity.
fn deploy_vault(w: &World) -> (Address, Address, Address) {
    let env = &w.env;
    let admin = Address::generate(env);
    let loan = token(env, &admin);
    let lender = Address::generate(env);
    StellarAssetClient::new(env, &loan).mint(&lender, &LENDER_LIQUIDITY);

    let vault_id = w.vault_id.clone();
    let v = vault::Client::new(env, &vault_id);
    v.initialize(&w.nova_id, &loan, &LTV_BPS, &BONUS_BPS);
    v.supply(&lender, &LENDER_LIQUIDITY);
    (vault_id, loan, lender)
}

#[test]
fn vault_refuses_collateral_before_market_is_active() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, _loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);

    assert!(v.try_deposit_collateral(&w.creator, &id, &COLLATERAL).is_err());
}

#[test]
fn vault_borrows_within_ltv_then_blocks_new_risk_when_degraded() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);

    // Collateral value = COLLATERAL at price 1.0. LTV 50% means the limit is COLLATERAL / 2.
    v.deposit_collateral(&w.creator, &id, &COLLATERAL);
    assert_eq!(v.position(&w.creator, &id).collateral, COLLATERAL);
    let limit = COLLATERAL * LTV_BPS as i128 / 10_000;

    v.borrow(&w.creator, &id, &limit);
    assert_eq!(v.position(&w.creator, &id).debt, limit);
    assert_eq!(token::Client::new(&w.env, &loan).balance(&w.creator), limit);
    assert!(v.try_borrow(&w.creator, &id, &1).is_err(), "over LTV must revert");

    // Degrade the market. New collateral and new borrows must now revert.
    degrade(&w, id);
    assert!(v.try_deposit_collateral(&w.creator, &id, &1_000).is_err());
    assert!(v.try_borrow(&w.creator, &id, &1).is_err());

    // Repaying still works, and collateral stays locked while the market is Degraded.
    assert_eq!(v.repay(&w.creator, &id, &limit), limit);
    assert_eq!(v.position(&w.creator, &id).debt, 0);
    assert!(v.try_withdraw_collateral(&w.creator, &id, &COLLATERAL).is_err());
}

#[test]
fn vault_releases_collateral_after_liquidity_returns() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, _loan, lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);

    v.deposit_collateral(&w.creator, &id, &COLLATERAL);
    degrade(&w, id);
    restore_liquidity(&w);
    w.refresh_oracle();
    assert_eq!(w.nova().checkpoint(&id), nova::MarketState::Active);

    // With no debt, all collateral comes back to the creator.
    v.withdraw_collateral(&w.creator, &id, &COLLATERAL);
    assert_eq!(v.position(&w.creator, &id).collateral, 0);
    assert_eq!(w.nova().balance(&id, &w.creator), SUPPLY);

    // The lender can take principal back out while the pool holds cash.
    v.withdraw_supply(&lender, &100_000);
    assert_eq!(v.supplied(&lender), LENDER_LIQUIDITY - 100_000);
}

// ---- Liquidation tests ----

fn set_oracle(w: &World, price: i128) {
    w.oracle().set_price(&price, &w.env.ledger().timestamp());
}

#[test]
fn liquidation_seizes_collateral_once_price_drops() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);

    v.deposit_collateral(&w.creator, &id, &COLLATERAL);
    v.borrow(&w.creator, &id, &50_000);

    // At price 0.6: limit = 100_000 * 0.6 * 50% = 30_000, debt 50_000 → unhealthy.
    set_oracle(&w, 6_000_000);

    let liquidator = Address::generate(&w.env);
    StellarAssetClient::new(&w.env, &loan).mint(&liquidator, &100_000);
    let (repaid, seized) = v.liquidate(&liquidator, &w.creator, &id, &60_000);

    // Close factor 50%: repay capped at 25_000.
    // seize = 25_000 * 1e7 / 6e6 * 10_500 / 10_000 = 41_666 * 10_500 / 10_000 = 43_749.
    assert_eq!(repaid, 25_000);
    assert_eq!(seized, 43_749);

    let pos = v.position(&w.creator, &id);
    assert_eq!(pos.debt, 25_000);
    assert_eq!(pos.collateral, COLLATERAL - 43_749);
    assert_eq!(w.nova().balance(&id, &liquidator), 43_749);
    assert_eq!(token::Client::new(&w.env, &loan).balance(&liquidator), 75_000);
}

#[test]
fn liquidation_rejects_healthy_position() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);

    v.deposit_collateral(&w.creator, &id, &COLLATERAL);
    v.borrow(&w.creator, &id, &50_000); // exactly at the limit at price 1.0

    let liquidator = Address::generate(&w.env);
    StellarAssetClient::new(&w.env, &loan).mint(&liquidator, &100_000);
    assert!(v
        .try_liquidate(&liquidator, &w.creator, &id, &10_000)
        .is_err());
}

#[test]
fn liquidation_settles_while_market_is_degraded() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);

    v.deposit_collateral(&w.creator, &id, &COLLATERAL);
    v.borrow(&w.creator, &id, &50_000);
    set_oracle(&w, 6_000_000);
    degrade(&w, id);

    // Settlement works while Degraded, and the same amounts as the Active case apply.
    let liquidator = Address::generate(&w.env);
    StellarAssetClient::new(&w.env, &loan).mint(&liquidator, &100_000);
    let (repaid, seized) = v.liquidate(&liquidator, &w.creator, &id, &60_000);
    assert_eq!(repaid, 25_000);
    assert_eq!(seized, 43_749);
    assert_eq!(w.nova().balance(&id, &liquidator), 43_749);

    // The liquidator's SBA is still subject to the open-trading halt.
    assert!(w.nova().try_transfer(&id, &liquidator, &w.creator, &1).is_err());
}

#[test]
fn liquidation_is_blocked_while_market_is_paused() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);

    v.deposit_collateral(&w.creator, &id, &COLLATERAL);
    v.borrow(&w.creator, &id, &50_000);
    set_oracle(&w, 6_000_000);

    // Let the oracle go stale and checkpoint, which pauses the market.
    w.env.ledger().set_timestamp(START + 86_401);
    assert_eq!(w.nova().checkpoint(&id), nova::MarketState::Paused);

    let liquidator = Address::generate(&w.env);
    StellarAssetClient::new(&w.env, &loan).mint(&liquidator, &100_000);
    assert!(v
        .try_liquidate(&liquidator, &w.creator, &id, &10_000)
        .is_err());
    assert_eq!(v.position(&w.creator, &id).debt, 50_000);
}

#[test]
fn settle_transfer_moves_only_the_settlers_own_balance() {
    let w = setup();
    let id = w.create_market();
    let (vault_id, _loan, _lender) = deploy_vault(&w);
    let v = vault::Client::new(&w.env, &vault_id);
    activate(&w, id);
    v.deposit_collateral(&w.creator, &id, &COLLATERAL);

    let outsider = Address::generate(&w.env);

    // A contract that is not on the settler allowlist cannot settle.
    assert!(w
        .nova()
        .try_settle_transfer(&id, &outsider, &outsider, &w.creator, &1)
        .is_err());

    // The vault cannot settle from a user's balance, only from its own.
    assert!(w
        .nova()
        .try_settle_transfer(&id, &vault_id, &w.creator, &outsider, &1)
        .is_err());

    // The vault can move its own collateral.
    w.nova().settle_transfer(&id, &vault_id, &vault_id, &outsider, &1);
    assert_eq!(w.nova().balance(&id, &outsider), 1);
}
