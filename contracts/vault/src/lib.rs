#![no_std]
//! A minimal share-based vault, fuzzed by Soroquake on every pull request.
//!
//! It keeps internal accounting only (no token transfers) so the example stays
//! small. The default build contains a deliberate bug: `withdraw` rounds the
//! payout UP, so once the share price is above 1 a user can withdraw more than
//! their fair share and slowly drain other depositors. Build with
//! `--features fixed` for the corrected version that rounds down.
//!
//! Invariants Soroquake should check:
//! - solvency: `total_assets >= sum of every user's fair claim`
//! - no free money: no user withdraws more than they deposited plus their share of yield

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env};

#[contracttype]
enum Key {
    Admin,
    TotalAssets,
    TotalShares,
    Shares(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NonPositiveAmount = 2,
    InsufficientShares = 3,
    InsufficientAssets = 4,
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&Key::Admin, &admin);
        env.storage().instance().set(&Key::TotalAssets, &0i128);
        env.storage().instance().set(&Key::TotalShares, &0i128);
    }

    /// Deposit `amount` assets and receive shares at the current price.
    pub fn deposit(env: Env, user: Address, amount: i128) -> i128 {
        user.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::NonPositiveAmount);
        }
        let (assets, shares) = (Self::total_assets(env.clone()), Self::total_shares(env.clone()));
        let minted = if shares == 0 || assets == 0 { amount } else { amount * shares / assets };
        Self::set_totals(&env, assets + amount, shares + minted);
        let key = Key::Shares(user);
        let bal: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(bal + minted));
        minted
    }

    /// Burn `shares` and receive assets at the current price.
    pub fn withdraw(env: Env, user: Address, shares: i128) -> i128 {
        user.require_auth();
        if shares <= 0 {
            panic_with_error!(&env, Error::NonPositiveAmount);
        }
        let key = Key::Shares(user);
        let bal: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if shares > bal {
            panic_with_error!(&env, Error::InsufficientShares);
        }
        let (assets, total) = (Self::total_assets(env.clone()), Self::total_shares(env.clone()));
        let payout = Self::payout(shares, assets, total);
        if payout > assets {
            panic_with_error!(&env, Error::InsufficientAssets);
        }
        env.storage().persistent().set(&key, &(bal - shares));
        Self::set_totals(&env, assets - payout, total - shares);
        payout
    }

    /// Admin-only: record yield, raising the share price for everyone.
    pub fn accrue(env: Env, amount: i128) {
        let admin: Address = env.storage().instance().get(&Key::Admin).unwrap();
        admin.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::NonPositiveAmount);
        }
        let (assets, shares) = (Self::total_assets(env.clone()), Self::total_shares(env.clone()));
        Self::set_totals(&env, assets + amount, shares);
    }

    pub fn shares(env: Env, user: Address) -> i128 {
        env.storage().persistent().get(&Key::Shares(user)).unwrap_or(0)
    }

    pub fn total_assets(env: Env) -> i128 {
        env.storage().instance().get(&Key::TotalAssets).unwrap_or(0)
    }

    pub fn total_shares(env: Env) -> i128 {
        env.storage().instance().get(&Key::TotalShares).unwrap_or(0)
    }
}

impl Vault {
    fn set_totals(env: &Env, assets: i128, shares: i128) {
        env.storage().instance().set(&Key::TotalAssets, &assets);
        env.storage().instance().set(&Key::TotalShares, &shares);
    }

    #[cfg(not(feature = "fixed"))]
    fn payout(shares: i128, assets: i128, total: i128) -> i128 {
        // BUG (intentional): rounds in the withdrawer's favour.
        (shares * assets + total - 1) / total
    }

    #[cfg(feature = "fixed")]
    fn payout(shares: i128, assets: i128, total: i128) -> i128 {
        shares * assets / total
    }
}

#[cfg(test)]
mod test;
