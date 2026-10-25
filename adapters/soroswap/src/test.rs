use super::*;
use soroban_sdk::{contract, contractimpl, symbol_short, testutils::Address as _};

#[contract]
pub struct MockRouter;

#[contractimpl]
impl MockRouter {
    pub fn set(env: Env, sell_out: i128, buy_in: i128, pair: Address) {
        env.storage().instance().set(&symbol_short!("OUT"), &sell_out);
        env.storage().instance().set(&symbol_short!("IN"), &buy_in);
        env.storage().instance().set(&symbol_short!("PAIR"), &pair);
    }

    pub fn router_get_amounts_out(env: Env, amount_in: i128, _path: Vec<Address>) -> Vec<i128> {
        let out: i128 = env.storage().instance().get(&symbol_short!("OUT")).unwrap();
        vec![&env, amount_in, out]
    }

    pub fn router_get_amounts_in(env: Env, amount_out: i128, _path: Vec<Address>) -> Vec<i128> {
        let input: i128 = env.storage().instance().get(&symbol_short!("IN")).unwrap();
        vec![&env, input, amount_out]
    }

    pub fn router_pair_for(env: Env, _a: Address, _b: Address) -> Address {
        env.storage().instance().get(&symbol_short!("PAIR")).unwrap()
    }
}

#[contract]
pub struct MockPair;

#[contractimpl]
impl MockPair {
    pub fn set(env: Env, token_0: Address, reserve_0: i128, reserve_1: i128) {
        env.storage().instance().set(&symbol_short!("T0"), &token_0);
        env.storage().instance().set(&symbol_short!("RES"), &(reserve_0, reserve_1));
    }

    pub fn get_reserves(env: Env) -> (i128, i128) {
        env.storage().instance().get(&symbol_short!("RES")).unwrap()
    }

    pub fn token_0(env: Env) -> Address {
        env.storage().instance().get(&symbol_short!("T0")).unwrap()
    }
}

struct Fixture {
    env: Env,
    adapter_id: Address,
    router_id: Address,
    pair_id: Address,
    base: Address,
    quote: Address,
}

impl Fixture {
    fn new(quote_is_token_0: bool) -> Self {
        let env = Env::default();
        let adapter_id = env.register(SoroswapAdapter, ());
        let router_id = env.register(MockRouter, ());
        let pair_id = env.register(MockPair, ());
        let base = Address::generate(&env);
        let quote = Address::generate(&env);

        let (token_0, reserve_0, reserve_1) = if quote_is_token_0 {
            (quote.clone(), 50_000, 40_000)
        } else {
            (base.clone(), 40_000, 50_000)
        };
        MockPairClient::new(&env, &pair_id).set(&token_0, &reserve_0, &reserve_1);
        MockRouterClient::new(&env, &router_id).set(&9_900, &10_100, &pair_id);

        SoroswapAdapterClient::new(&env, &adapter_id)
            .initialize(&router_id, &base, &quote, &1_000);

        Fixture {
            env,
            adapter_id,
            router_id,
            pair_id,
            base,
            quote,
        }
    }

    fn adapter(&self) -> SoroswapAdapterClient<'_> {
        SoroswapAdapterClient::new(&self.env, &self.adapter_id)
    }
}

#[test]
fn snapshot_reads_bid_ask_and_quote_depth() {
    let f = Fixture::new(true);
    let snap = f.adapter().snapshot(&0);
    assert_eq!(
        snap,
        LiquiditySnapshot {
            bid: 9_900,
            ask: 10_100,
            depth: 50_000,
        }
    );
}

#[test]
fn depth_uses_reserve_one_when_quote_is_token_one() {
    let f = Fixture::new(false);
    assert_eq!(f.adapter().snapshot(&0).depth, 50_000);
}

#[test]
fn config_is_bound_at_initialize() {
    let f = Fixture::new(true);
    let cfg = f.adapter().config();
    assert_eq!(cfg.router, f.router_id);
    assert_eq!(cfg.base, f.base);
    assert_eq!(cfg.quote, f.quote);
    assert_eq!(cfg.probe_base, 1_000);
    assert_eq!(f.pair_id, MockRouterClient::new(&f.env, &f.router_id).router_pair_for(&f.base, &f.quote));
}

#[test]
fn initialize_is_one_shot() {
    let f = Fixture::new(true);
    assert!(f
        .adapter()
        .try_initialize(&f.router_id, &f.base, &f.quote, &1_000)
        .is_err());
}

#[test]
fn snapshot_fails_before_initialize() {
    let env = Env::default();
    let adapter_id = env.register(SoroswapAdapter, ());
    let adapter = SoroswapAdapterClient::new(&env, &adapter_id);
    assert!(adapter.try_snapshot(&0).is_err());
}

#[test]
fn initialize_rejects_identical_tokens_and_bad_probe() {
    let env = Env::default();
    let adapter_id = env.register(SoroswapAdapter, ());
    let adapter = SoroswapAdapterClient::new(&env, &adapter_id);
    let t = Address::generate(&env);
    let r = Address::generate(&env);
    assert!(adapter.try_initialize(&r, &t, &t, &1_000).is_err());
    let u = Address::generate(&env);
    assert!(adapter.try_initialize(&r, &t, &u, &0).is_err());
}
