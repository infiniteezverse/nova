#![no_std]
//! TEST-ONLY SEP-40-shaped price feed for NOVA's Stellar testnet deployment.
//!
//! This is not a real price source. Its price is set by a single admin address,
//! so it proves the NOVA wiring and nothing about market data. Never put this
//! contract on a production oracle allowlist.

use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, Symbol};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Asset {
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
pub struct TestOracle;

#[contractimpl]
impl TestOracle {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("ADMIN")) {
            panic!("already initialized");
        }
        env.storage().instance().set(&symbol_short!("ADMIN"), &admin);
    }

    /// Admin-only. Sets the price and timestamp that `lastprice` returns for every asset.
    pub fn set_price(env: Env, price: i128, timestamp: u64) {
        let admin: Address = env.storage().instance().get(&symbol_short!("ADMIN")).unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("PX"), &PriceData { price, timestamp });
    }

    pub fn lastprice(env: Env, _asset: Asset) -> Option<PriceData> {
        env.storage().instance().get(&symbol_short!("PX"))
    }
}
