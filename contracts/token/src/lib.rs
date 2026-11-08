#![no_std]
//! A small SEP-41 style token: mint, transfer, approve, transfer_from, burn.
//!
//! Soroquake checks that total supply always equals the sum of balances and
//! that allowances only ever shrink by exactly what was spent.

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env};

#[contracttype]
enum Key {
    Admin,
    Supply,
    Balance(Address),
    Allowance(Address, Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NonPositiveAmount = 1,
    InsufficientBalance = 2,
    InsufficientAllowance = 3,
    NegativeAllowance = 4,
}

#[contract]
pub struct Token;

#[contractimpl]
impl Token {
    pub fn __constructor(env: Env, admin: Address) {
        env.storage().instance().set(&Key::Admin, &admin);
        env.storage().instance().set(&Key::Supply, &0i128);
    }

    /// Admin-only: create `amount` new tokens for `to`.
    pub fn mint(env: Env, to: Address, amount: i128) {
        let admin: Address = env.storage().instance().get(&Key::Admin).unwrap();
        admin.require_auth();
        Self::positive(&env, amount);
        Self::credit(&env, &to, amount);
        let supply = Self::total_supply(env.clone());
        env.storage().instance().set(&Key::Supply, &(supply + amount));
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        Self::positive(&env, amount);
        Self::debit(&env, &from, amount);
        Self::credit(&env, &to, amount);
    }

    /// Let `spender` move up to `amount` of `from`'s tokens. Replaces any
    /// previous allowance.
    pub fn approve(env: Env, from: Address, spender: Address, amount: i128) {
        from.require_auth();
        if amount < 0 {
            panic_with_error!(&env, Error::NegativeAllowance);
        }
        env.storage().persistent().set(&Key::Allowance(from, spender), &amount);
    }

    pub fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        Self::positive(&env, amount);
        let key = Key::Allowance(from.clone(), spender);
        let allowed: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if amount > allowed {
            panic_with_error!(&env, Error::InsufficientAllowance);
        }
        Self::debit(&env, &from, amount);
        env.storage().persistent().set(&key, &(allowed - amount));
        Self::credit(&env, &to, amount);
    }

    pub fn burn(env: Env, from: Address, amount: i128) {
        from.require_auth();
        Self::positive(&env, amount);
        Self::debit(&env, &from, amount);
        let supply = Self::total_supply(env.clone());
        env.storage().instance().set(&Key::Supply, &(supply - amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage().persistent().get(&Key::Balance(id)).unwrap_or(0)
    }

    pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
        env.storage().persistent().get(&Key::Allowance(from, spender)).unwrap_or(0)
    }

    pub fn total_supply(env: Env) -> i128 {
        env.storage().instance().get(&Key::Supply).unwrap_or(0)
    }
}

impl Token {
    fn positive(env: &Env, amount: i128) {
        if amount <= 0 {
            panic_with_error!(env, Error::NonPositiveAmount);
        }
    }

    fn credit(env: &Env, id: &Address, amount: i128) {
        let key = Key::Balance(id.clone());
        let bal: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(bal + amount));
    }

    fn debit(env: &Env, id: &Address, amount: i128) {
        let key = Key::Balance(id.clone());
        let bal: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if amount > bal {
            panic_with_error!(env, Error::InsufficientBalance);
        }
        env.storage().persistent().set(&key, &(bal - amount));
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn mint_transfer_and_allowance() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let (alice, bob) = (Address::generate(&env), Address::generate(&env));
        let token = TokenClient::new(&env, &env.register(Token, (admin,)));
        token.mint(&alice, &100);
        token.transfer(&alice, &bob, &30);
        token.approve(&alice, &bob, &20);
        token.transfer_from(&bob, &alice, &bob, &15);
        assert_eq!(token.balance(&alice), 55);
        assert_eq!(token.balance(&bob), 45);
        assert_eq!(token.allowance(&alice, &bob), 5);
        assert!(token.try_transfer_from(&bob, &alice, &bob, &6).is_err());
        assert_eq!(token.total_supply(), 100);
    }
}
