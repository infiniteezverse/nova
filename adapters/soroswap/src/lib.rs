#![no_std]
//! Soroswap liquidity adapter for NOVA.
//!
//! One adapter instance serves one token pair, fixed at `initialize`. That keeps the
//! adapter admin-free. NOVA's allowlist decides which adapter instances a market may use.
//!
//! `snapshot` returns the `LiquiditySnapshot` NOVA's constitution reads:
//! - `bid`: quote received for selling `probe_base` into the pool (router out-quote)
//! - `ask`: quote required to buy `probe_base` out of the pool (router in-quote)
//! - `depth`: quote-side reserve of the pair
//!
//! Router and pair signatures come from github.com/soroswap/core
//! (contracts/router/src/lib.rs, contracts/pair/src/lib.rs).

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    vec, Address, Env, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidParams = 3,
}

/// Same XDR shape as NOVA's `LiquiditySnapshot`.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LiquiditySnapshot {
    pub bid: i128,
    pub ask: i128,
    pub depth: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub router: Address,
    pub base: Address,
    pub quote: Address,
    pub probe_base: i128,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Config,
}

#[contractclient(name = "SoroswapRouterClient")]
pub trait SoroswapRouter {
    fn router_get_amounts_out(env: Env, amount_in: i128, path: Vec<Address>) -> Vec<i128>;
    fn router_get_amounts_in(env: Env, amount_out: i128, path: Vec<Address>) -> Vec<i128>;
    fn router_pair_for(env: Env, token_a: Address, token_b: Address) -> Address;
}

#[contractclient(name = "SoroswapPairClient")]
pub trait SoroswapPair {
    fn get_reserves(env: Env) -> (i128, i128);
    fn token_0(env: Env) -> Address;
}

#[contract]
pub struct SoroswapAdapter;

#[contractimpl]
impl SoroswapAdapter {
    /// Bind this adapter to one pair. Runs once. No function changes the pair afterward.
    pub fn initialize(env: Env, router: Address, base: Address, quote: Address, probe_base: i128) {
        if env.storage().instance().has(&DataKey::Config) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        if probe_base <= 0 || base == quote {
            panic_with_error!(&env, Error::InvalidParams);
        }
        env.storage().instance().set(
            &DataKey::Config,
            &Config {
                router,
                base,
                quote,
                probe_base,
            },
        );
    }

    pub fn config(env: Env) -> Config {
        load_config(&env)
    }

    /// Read-only. Probes the router for a two-sided quote at `probe_base` size.
    /// `market_id` is unused: this adapter serves one pair for every market that allowlists it.
    pub fn snapshot(env: Env, _market_id: u64) -> LiquiditySnapshot {
        let cfg = load_config(&env);
        let router = SoroswapRouterClient::new(&env, &cfg.router);

        let sell_path = vec![&env, cfg.base.clone(), cfg.quote.clone()];
        let buy_path = vec![&env, cfg.quote.clone(), cfg.base.clone()];
        let bid = router
            .router_get_amounts_out(&cfg.probe_base, &sell_path)
            .get(1)
            .unwrap_or(0);
        let ask = router
            .router_get_amounts_in(&cfg.probe_base, &buy_path)
            .get(0)
            .unwrap_or(0);

        let pair = SoroswapPairClient::new(
            &env,
            &router.router_pair_for(&cfg.base, &cfg.quote),
        );
        let (reserve_0, reserve_1) = pair.get_reserves();
        let depth = if pair.token_0() == cfg.quote {
            reserve_0
        } else {
            reserve_1
        };

        LiquiditySnapshot { bid, ask, depth }
    }
}

fn load_config(env: &Env) -> Config {
    env.storage()
        .instance()
        .get(&DataKey::Config)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

#[cfg(test)]
mod test;
