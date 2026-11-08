use super::*;
use soroban_sdk::{testutils::Address as _, Env};

fn setup() -> (Env, VaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let id = env.register(Vault, (admin.clone(),));
    let client = VaultClient::new(&env, &id);
    (env, client, admin)
}

#[test]
fn deposit_then_withdraw_at_par() {
    let (env, vault, _) = setup();
    let alice = Address::generate(&env);
    assert_eq!(vault.deposit(&alice, &100), 100);
    assert_eq!(vault.withdraw(&alice, &100), 100);
    assert_eq!(vault.total_assets(), 0);
}

#[test]
fn rejects_invalid_amounts() {
    let (env, vault, _) = setup();
    let alice = Address::generate(&env);
    vault.deposit(&alice, &10);
    assert!(vault.try_withdraw(&alice, &11).is_err());
    assert!(vault.try_deposit(&alice, &0).is_err());
}

/// The planted bug: with 3 shares backed by 4 assets, burning 1 share should
/// pay floor(4/3) = 1, but the buggy build pays 2.
#[test]
fn rounding_on_withdraw() {
    let (env, vault, _) = setup();
    let alice = Address::generate(&env);
    vault.deposit(&alice, &3);
    vault.accrue(&1);
    let paid = vault.withdraw(&alice, &1);
    #[cfg(not(feature = "fixed"))]
    assert_eq!(paid, 2, "buggy build rounds up");
    #[cfg(feature = "fixed")]
    assert_eq!(paid, 1, "fixed build rounds down");
}
