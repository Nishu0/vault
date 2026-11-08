//! Soroquake campaign for the vault and token contracts.
//!
//! Three users and an admin make random calls against both contracts. After
//! every call, every invariant below is checked. When one breaks, Soroquake
//! shrinks the sequence to the fewest calls that still break it.
//!
//!   SOROQUAKE_TIME=60 cargo test -p invariants --profile fuzz -- --nocapture

use soroban_sdk::{
    testutils::{Address as _, EnvTestConfig},
    Address, Env,
};
use soroquake::prelude::*;
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

impl Harness for Campaign {
    type State = World;
    type Op = Op;

    fn setup(&self) -> World {
        // No snapshot files: thousands of short-lived environments per campaign.
        let env = Env::new_with_config(EnvTestConfig { capture_snapshot_at_drop: false });
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = VaultClient::new(&env, &env.register(Vault, (admin.clone(),)));
        let token = TokenClient::new(&env, &env.register(Token, (admin,)));
        let users = (0..USERS.len()).map(|_| Address::generate(&env)).collect();
        World { vault, token, users, prev: (0, 0), ghost_allowance: [[0; 3]; 3] }
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
                let note = r.as_ref().ok().and_then(|x| x.as_ref().ok()).map(|m| format!("minted {m} shares"));
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
            Invariant::new("shares_sum_to_total", "vault", |w| {
                let sum: i128 = w.users.iter().map(|u| w.vault.shares(u)).sum();
                let total = w.vault.total_shares();
                check!(sum == total, "user shares add to {sum}, total_shares is {total}")
            }),
            Invariant::new("supply_matches_balances", "sep41", |w| {
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
