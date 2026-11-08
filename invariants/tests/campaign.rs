//! Soroquake campaign for the vault and token contracts.
//!
//! Three users and an admin make random calls against both contracts. After
//! every call, every invariant below is checked. When one breaks, Soroquake
//! shrinks the sequence to the fewest calls that still break it.
//!
//!   SOROQUAKE_TIME=60 cargo test -p invariants --profile fuzz -- --nocapture
//!
//! Fork mode starts every sequence from real testnet state instead, running
//! the deployed wasm with the real balances (refresh with scripts/snapshot.sh):
//!
//!   SOROQUAKE_SNAPSHOT=snapshots/testnet.json cargo test -p invariants --profile fuzz -- --nocapture

use soroban_sdk::{
    testutils::{Address as _, EnvTestConfig},
    Address, Env,
};
use soroquake::prelude::*;
use soroquake::Fork;
use std::sync::OnceLock;
use token::{Token, TokenClient};
use vault::{Vault, VaultClient};

const USERS: [&str; 3] = ["alice", "bob", "carol"];

struct World {
    vault: VaultClient<'static>,
    token: TokenClient<'static>,
    users: Vec<Address>,
    /// Vault (assets, shares) before the last call, for the price check.
    prev: (i128, i128),
    /// What each (owner, spender) allowance should be, tracked independently.
    ghost_allowance: [[i128; 3]; 3],
    /// Set when a deposit succeeded but minted no shares.
    zero_mint: Option<(usize, i128)>,
}

#[derive(Clone, Debug)]
enum Op {
    Deposit(usize, i128),
    Withdraw(usize, i128),
    Accrue(i128),
    Mint(usize, i128),
    Transfer(usize, usize, i128),
    Approve(usize, usize, i128),
    TransferFrom(usize, usize, usize, i128),
    Burn(usize, i128),
}

fn outcome<T, E: core::fmt::Debug, C, I: core::fmt::Debug>(r: &Result<Result<T, C>, Result<E, I>>) -> Result<(), String> {
    match r {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err("conversion error".into()),
        Err(Ok(e)) => Err(format!("{e:?}")),
        Err(Err(e)) => Err(format!("{e:?}")),
    }
}

struct Campaign;

fn fork() -> Option<&'static Fork> {
    static FORK: OnceLock<Option<Fork>> = OnceLock::new();
    FORK.get_or_init(Fork::from_env).as_ref()
}

/// Real testnet state: the deployed vault and token, alice and bob's actual
/// positions, plus a fresh account for carol.
fn forked_world(f: &Fork) -> World {
    let mut env = Env::from_ledger_snapshot_file(&f.path);
    env.set_config(EnvTestConfig { capture_snapshot_at_drop: false });
    env.mock_all_auths();
    let addr = |s: Option<&str>, what: &str| Address::from_str(&env, s.unwrap_or_else(|| panic!("snapshot meta has no {what}")));
    let vault = VaultClient::new(&env, &addr(f.contract("vault"), "vault"));
    let token = TokenClient::new(&env, &addr(f.contract("token"), "token"));
    let users = vec![addr(f.actor("alice"), "alice"), addr(f.actor("bob"), "bob"), Address::generate(&env)];
    let mut ghost_allowance = [[0; 3]; 3];
    for (i, from) in users.iter().enumerate() {
        for (j, spender) in users.iter().enumerate() {
            ghost_allowance[i][j] = token.allowance(from, spender);
        }
    }
    let prev = (vault.total_assets(), vault.total_shares());
    World { vault, token, users, prev, ghost_allowance, zero_mint: None }
}

impl Harness for Campaign {
    type State = World;
    type Op = Op;

    fn setup(&self) -> World {
        if let Some(f) = fork() {
            return forked_world(f);
        }
        // No snapshot files: thousands of short-lived environments per campaign.
        let env = Env::new_with_config(EnvTestConfig { capture_snapshot_at_drop: false });
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = VaultClient::new(&env, &env.register(Vault, (admin.clone(),)));
        let token = TokenClient::new(&env, &env.register(Token, (admin,)));
        let users = (0..USERS.len()).map(|_| Address::generate(&env)).collect();
        World { vault, token, users, prev: (0, 0), ghost_allowance: [[0; 3]; 3], zero_mint: None }
    }

    fn generate(&self, w: &World, rng: &mut SeededRng) -> Op {
        let u = rng.below(3) as usize;
        let v = rng.below(3) as usize;
        let small = rng.range(1, 12);
        match rng.below(12) {
            0..=2 => Op::Deposit(u, small),
            3..=4 => {
                let held = w.vault.shares(&w.users[u]);
                Op::Withdraw(u, rng.range(1, held.max(1)))
            }
            5 => Op::Accrue(rng.range(1, 5)),
            6 => Op::Mint(u, small * 10),
            7 => Op::Transfer(u, v, small),
            8 => Op::Approve(u, v, rng.range(0, 30)),
            9..=10 => Op::TransferFrom(v, u, rng.below(3) as usize, small),
            _ => Op::Burn(u, small),
        }
    }

    fn apply(&self, w: &mut World, op: &Op) -> Step {
        w.prev = (w.vault.total_assets(), w.vault.total_shares());
        let user = |i: usize| w.users[i].clone();
        let (step, result) = match *op {
            Op::Deposit(u, amt) => {
                let r = w.vault.try_deposit(&user(u), &amt);
                let minted = r.as_ref().ok().and_then(|x| x.as_ref().ok()).copied();
                if minted == Some(0) {
                    w.zero_mint = Some((u, amt));
                }
                let note = minted.map(|m| format!("minted {m} shares"));
                (Step::new(USERS[u], format!("deposit({amt})")).note(note.unwrap_or_default()), outcome(&r))
            }
            Op::Withdraw(u, shares) => {
                let r = w.vault.try_withdraw(&user(u), &shares);
                let note = r.as_ref().ok().and_then(|x| x.as_ref().ok()).map(|p| format!("paid {p}"));
                (Step::new(USERS[u], format!("withdraw({shares})")).note(note.unwrap_or_default()), outcome(&r))
            }
            Op::Accrue(amt) => {
                let r = w.vault.try_accrue(&amt);
                (Step::new("admin", format!("accrue({amt})")), outcome(&r))
            }
            Op::Mint(u, amt) => {
                let r = w.token.try_mint(&user(u), &amt);
                (Step::new("admin", format!("mint({}, {amt})", USERS[u])), outcome(&r))
            }
            Op::Transfer(u, v, amt) => {
                let r = w.token.try_transfer(&user(u), &user(v), &amt);
                (Step::new(USERS[u], format!("transfer({}, {amt})", USERS[v])), outcome(&r))
            }
            Op::Approve(u, v, amt) => {
                let r = w.token.try_approve(&user(u), &user(v), &amt);
                if outcome(&r).is_ok() {
                    w.ghost_allowance[u][v] = amt;
                }
                (Step::new(USERS[u], format!("approve({}, {amt})", USERS[v])), outcome(&r))
            }
            Op::TransferFrom(s, from, to, amt) => {
                let r = w.token.try_transfer_from(&user(s), &user(from), &user(to), &amt);
                if outcome(&r).is_ok() {
                    w.ghost_allowance[from][s] -= amt;
                }
                (Step::new(USERS[s], format!("transfer_from({}, {}, {amt})", USERS[from], USERS[to])), outcome(&r))
            }
            Op::Burn(u, amt) => {
                let r = w.token.try_burn(&user(u), &amt);
                (Step::new(USERS[u], format!("burn({amt})")), outcome(&r))
            }
        };
        let name = step.call.split('(').next().unwrap_or("").to_string();
        match result {
            Ok(()) => step.branch(format!("{name}:ok")),
            Err(e) => {
                let note = format!("rejected: {e}");
                step.note(note).branch(format!("{name}:err"))
            }
        }
    }

    fn invariants(&self) -> Vec<Invariant<World>> {
        vec![
            // Withdrawals and deposits must never make existing shares worth less.
            Invariant::new("share_price_never_drops", "vault", |w| {
                let (a0, s0) = w.prev;
                let (a1, s1) = (w.vault.total_assets(), w.vault.total_shares());
                if s0 == 0 || s1 == 0 {
                    return Check::Pass;
                }
                check!(a1 * s0 >= a0 * s1, "share price fell from {a0}/{s0} to {a1}/{s1}")
            }),
            // Outstanding shares must always be backed by something.
            Invariant::new("solvent", "vault", |w| {
                let (a, s) = (w.vault.total_assets(), w.vault.total_shares());
                check!(s == 0 || a > 0, "{s} shares outstanding but 0 assets")
            }),
            // Every accepted deposit must leave the depositor with something.
            Invariant::new("deposits_mint_shares", "vault", |w| match w.zero_mint {
                Some((u, amt)) => Check::Fail(format!("{} deposited {amt} and received 0 shares", USERS[u])),
                None => Check::Pass,
            }),
            Invariant::new("shares_sum_to_total", "vault", |w| {
                let sum: i128 = w.users.iter().map(|u| w.vault.shares(u)).sum();
                let total = w.vault.total_shares();
                check!(sum == total, "user shares add to {sum}, total_shares is {total}")
            }),
            Invariant::new("supply_matches_balances", "sep41", |w| {
                // Only these three accounts ever hold tokens, on testnet too.
                let sum: i128 = w.users.iter().map(|u| w.token.balance(u)).sum();
                let supply = w.token.total_supply();
                check!(sum == supply, "balances add to {sum}, total_supply is {supply}")
            }),
            Invariant::new("allowance_never_overspent", "sep41", |w| {
                for (i, from) in w.users.iter().enumerate() {
                    for (j, spender) in w.users.iter().enumerate() {
                        let got = w.token.allowance(from, spender);
                        let want = w.ghost_allowance[i][j];
                        if got != want || got < 0 {
                            return Check::Fail(format!("{}→{} allowance is {got}, expected {want}", USERS[i], USERS[j]));
                        }
                    }
                }
                Check::Pass
            }),
            Invariant::new("no_negative_balances", "sep41", |w| {
                match w.users.iter().position(|u| w.token.balance(u) < 0) {
                    Some(i) => Check::Fail(format!("{} has a negative balance", USERS[i])),
                    None => Check::Pass,
                }
            }),
        ]
    }

    /// Replay lab: alice is the victim, bob the attacker. Assets that reach the
    /// vault without minting shares are modelled with the admin's `accrue`.
    fn lab_op(&self, w: &World, action: &LabAction) -> Option<Op> {
        let who = |a: &Actor| match a {
            Actor::Victim => 0,
            Actor::Attacker => 1,
            Actor::Admin => 2,
        };
        Some(match action {
            LabAction::Deposit(a, amt) => Op::Deposit(who(a), *amt),
            LabAction::Withdraw(a, shares) => Op::Withdraw(who(a), *shares),
            LabAction::WithdrawAll(a) => Op::Withdraw(who(a), w.vault.shares(&w.users[who(a)]).max(1)),
            LabAction::Donate(_, amt) => Op::Accrue(*amt),
            LabAction::Mint(a, amt) => Op::Mint(who(a), *amt),
            LabAction::Transfer(a, b, amt) => Op::Transfer(who(a), who(b), *amt),
            LabAction::Approve(a, b, amt) => Op::Approve(who(a), who(b), *amt),
            LabAction::TransferFrom { spender, from, to, amount } => Op::TransferFrom(who(spender), who(from), who(to), *amount),
            LabAction::Burn(a, amt) => Op::Burn(who(a), *amt),
        })
    }

    fn branches(&self) -> Vec<&'static str> {
        vec![
            "deposit:ok", "deposit:err", "withdraw:ok", "withdraw:err", "accrue:ok",
            "mint:ok", "transfer:ok", "transfer:err", "approve:ok",
            "transfer_from:ok", "transfer_from:err", "burn:ok", "burn:err",
        ]
    }
}

#[test]
fn soroquake() {
    soroquake::run(Campaign);
}
