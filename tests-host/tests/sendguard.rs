use litesvm::LiteSVM;
use litesvm::types::TransactionResult;
use sha2::{Digest, Sha256};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use std::str::FromStr;

const IDL: &str = include_str!("../../idl/sendguard.json");
const AMOUNT: u64 = 500_000_000;
const SECRET: [u8; 32] = [7; 32];
const HOLD: i64 = 60;
const NONCE: u64 = 1;

fn idl() -> serde_json::Value {
    serde_json::from_str(IDL).unwrap()
}

fn discriminator(kind: &str, name: &str) -> Vec<u8> {
    let list = if kind == "account" { "accounts" } else { "instructions" };
    idl()[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not in the IDL"))["discriminator"]
        .as_array()
        .unwrap()
        .iter()
        .map(|byte| byte.as_u64().unwrap() as u8)
        .collect()
}

struct Lab {
    svm: LiteSVM,
    program: Pubkey,
    sender: Keypair,
    friend: Keypair,
    scammer: Keypair,
    payment: Pubkey,
}

fn lab() -> Lab {
    let mut svm = LiteSVM::new().with_transaction_history(0);
    let program = Pubkey::from_str(idl()["address"].as_str().unwrap()).unwrap();
    let elf = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/deploy/sendguard.so"
    ))
    .expect("build the program first");
    svm.add_program(program, &elf).unwrap();
    let (sender, friend, scammer) = (Keypair::new(), Keypair::new(), Keypair::new());
    for who in [&sender, &friend, &scammer] {
        svm.airdrop(&who.pubkey(), 5_000_000_000).unwrap();
    }
    let (payment, _) = Pubkey::find_program_address(
        &[b"payment", sender.pubkey().as_ref(), &NONCE.to_le_bytes()],
        &program,
    );
    Lab { svm, program, sender, friend, scammer, payment }
}

impl Lab {
    fn submit(&mut self, ix: Instruction, signer: &Keypair) -> TransactionResult {
        self.svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&signer.pubkey()),
            &[signer],
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx)
    }

    fn send_with(&mut self, recipient: Pubkey, amount: u64, hold: i64) -> TransactionResult {
        let mut data = discriminator("global", "send");
        data.extend_from_slice(&NONCE.to_le_bytes());
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&hold.to_le_bytes());
        data.extend_from_slice(&Sha256::digest(SECRET));
        let ix = Instruction::new_with_bytes(
            self.program,
            &data,
            vec![
                AccountMeta::new(self.sender.pubkey(), true),
                AccountMeta::new(self.payment, false),
                AccountMeta::new_readonly(recipient, false),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            ],
        );
        let sender = self.sender.insecure_clone();
        self.submit(ix, &sender)
    }

    fn send_to(&mut self, recipient: Pubkey) -> TransactionResult {
        self.send_with(recipient, AMOUNT, HOLD)
    }

    fn claim_on(&mut self, who: &Keypair, payment: Pubkey, secret: [u8; 32]) -> TransactionResult {
        let mut data = discriminator("global", "claim");
        data.extend_from_slice(&secret);
        let ix = Instruction::new_with_bytes(
            self.program,
            &data,
            vec![
                AccountMeta::new(who.pubkey(), true),
                AccountMeta::new(payment, false),
                AccountMeta::new(self.sender.pubkey(), false),
            ],
        );
        self.submit(ix, who)
    }

    fn claim(&mut self, who: &Keypair, secret: [u8; 32]) -> TransactionResult {
        let payment = self.payment;
        self.claim_on(who, payment, secret)
    }

    fn reclaim(&mut self, who: &Keypair) -> TransactionResult {
        let ix = Instruction::new_with_bytes(
            self.program,
            &discriminator("global", "reclaim"),
            vec![
                AccountMeta::new_readonly(who.pubkey(), true),
                AccountMeta::new(self.payment, false),
                AccountMeta::new(self.sender.pubkey(), false),
            ],
        );
        self.submit(ix, who)
    }

    fn balance(&self, who: &Pubkey) -> u64 {
        self.svm.get_balance(who).unwrap_or(0)
    }

    fn pass_deadline(&mut self) {
        let mut clock = self.svm.get_sysvar::<solana_clock::Clock>();
        clock.unix_timestamp += HOLD + 1;
        self.svm.set_sysvar(&clock);
    }

    fn logs(result: &TransactionResult) -> String {
        result.as_ref().err().expect("should fail").meta.logs.join("\n")
    }
}

#[test]
fn right_wallet_with_right_secret_gets_the_money() {
    let mut l = lab();
    let friend = l.friend.insecure_clone();
    let before = l.balance(&friend.pubkey());
    l.send_to(friend.pubkey()).unwrap();
    l.claim(&friend, SECRET).unwrap();
    assert_eq!(l.balance(&friend.pubkey()), before + AMOUNT - 5000);
    assert_eq!(l.balance(&l.payment), 0);
}

#[test]
fn claim_returns_the_rent_to_the_sender() {
    let mut l = lab();
    let (friend, sender) = (l.friend.insecure_clone(), l.sender.insecure_clone());
    let before = l.balance(&sender.pubkey());
    l.send_to(friend.pubkey()).unwrap();
    l.claim(&friend, SECRET).unwrap();
    assert_eq!(l.balance(&sender.pubkey()), before - AMOUNT - 5000);
}

#[test]
fn scammer_who_owns_the_address_cannot_claim_without_the_secret() {
    let mut l = lab();
    let scammer = l.scammer.insecure_clone();
    l.send_to(scammer.pubkey()).unwrap();
    let held = l.balance(&l.payment);
    let result = l.claim(&scammer, [9; 32]);
    assert!(Lab::logs(&result).contains("WrongSecret"));
    assert_eq!(l.balance(&l.payment), held);
}

#[test]
fn stolen_secret_is_useless_from_another_wallet() {
    let mut l = lab();
    let (friend, scammer) = (l.friend.insecure_clone(), l.scammer.insecure_clone());
    l.send_to(friend.pubkey()).unwrap();
    assert!(Lab::logs(&l.claim(&scammer, SECRET)).contains("NotRecipient"));
}

#[test]
fn nobody_can_claim_after_the_hold_ends() {
    let mut l = lab();
    let friend = l.friend.insecure_clone();
    l.send_to(friend.pubkey()).unwrap();
    l.pass_deadline();
    assert!(Lab::logs(&l.claim(&friend, SECRET)).contains("HoldExpired"));
}

#[test]
fn sender_can_cancel_early_and_gets_everything_back() {
    let mut l = lab();
    let (friend, sender) = (l.friend.insecure_clone(), l.sender.insecure_clone());
    let before = l.balance(&sender.pubkey());
    l.send_to(friend.pubkey()).unwrap();
    l.reclaim(&sender).unwrap();
    assert_eq!(l.balance(&sender.pubkey()), before - 10_000);
    assert!(l.claim(&friend, SECRET).is_err());
}

#[test]
fn only_the_sender_can_cancel_before_the_hold_ends() {
    let mut l = lab();
    let scammer = l.scammer.insecure_clone();
    l.send_to(scammer.pubkey()).unwrap();
    assert!(Lab::logs(&l.reclaim(&scammer)).contains("HoldStillActive"));
}

#[test]
fn anyone_can_trigger_the_refund_but_it_only_goes_to_the_sender() {
    let mut l = lab();
    let (scammer, sender) = (l.scammer.insecure_clone(), l.sender.insecure_clone());
    let before = l.balance(&sender.pubkey());
    let scammer_before = l.balance(&scammer.pubkey());
    l.send_to(scammer.pubkey()).unwrap();
    l.pass_deadline();
    l.reclaim(&scammer).unwrap();
    assert_eq!(l.balance(&sender.pubkey()), before - 5000);
    assert_eq!(l.balance(&scammer.pubkey()), scammer_before - 5000);
}

#[test]
fn claim_cannot_redirect_the_rent_to_a_different_sender() {
    let mut l = lab();
    let (friend, scammer) = (l.friend.insecure_clone(), l.scammer.insecure_clone());
    l.send_to(friend.pubkey()).unwrap();
    let mut data = discriminator("global", "claim");
    data.extend_from_slice(&SECRET);
    let ix = Instruction::new_with_bytes(
        l.program,
        &data,
        vec![
            AccountMeta::new(friend.pubkey(), true),
            AccountMeta::new(l.payment, false),
            AccountMeta::new(scammer.pubkey(), false),
        ],
    );
    assert!(l.submit(ix, &friend).is_err());
}

#[test]
fn a_closed_payment_cannot_be_paid_out_twice() {
    let mut l = lab();
    let (friend, sender) = (l.friend.insecure_clone(), l.sender.insecure_clone());
    l.send_to(friend.pubkey()).unwrap();
    l.claim(&friend, SECRET).unwrap();
    assert!(l.claim(&friend, SECRET).is_err());
    assert!(l.reclaim(&sender).is_err());
}

#[test]
fn prefunding_the_payment_address_cannot_block_a_send() {
    let mut l = lab();
    let friend = l.friend.insecure_clone();
    let payment = l.payment;
    l.svm.airdrop(&payment, 1_000_000).unwrap();
    l.send_to(friend.pubkey()).unwrap();
    let before = l.balance(&friend.pubkey());
    l.claim(&friend, SECRET).unwrap();
    assert_eq!(l.balance(&friend.pubkey()), before + AMOUNT - 5000);
}

#[test]
fn the_same_nonce_cannot_overwrite_an_open_payment() {
    let mut l = lab();
    let (friend, scammer) = (l.friend.insecure_clone(), l.scammer.insecure_clone());
    l.send_to(friend.pubkey()).unwrap();
    assert!(l.send_to(scammer.pubkey()).is_err());
    l.claim(&friend, SECRET).unwrap();
}

#[test]
fn zero_amount_and_bad_hold_times_are_rejected() {
    let mut l = lab();
    let friend = l.friend.pubkey();
    assert!(Lab::logs(&l.send_with(friend, 0, HOLD)).contains("InvalidAmount"));
    assert!(Lab::logs(&l.send_with(friend, AMOUNT, 0)).contains("InvalidHold"));
    assert!(Lab::logs(&l.send_with(friend, AMOUNT, 31 * 24 * 60 * 60)).contains("InvalidHold"));
}

#[test]
fn a_forged_payment_account_is_rejected() {
    let mut l = lab();
    let scammer = l.scammer.insecure_clone();
    let sender = l.sender.pubkey();
    let mut data = discriminator("account", "Payment");
    data.extend_from_slice(sender.as_ref());
    data.extend_from_slice(scammer.pubkey().as_ref());
    data.extend_from_slice(&Sha256::digest(SECRET));
    data.extend_from_slice(&AMOUNT.to_le_bytes());
    data.extend_from_slice(&i64::MAX.to_le_bytes());
    data.extend_from_slice(&NONCE.to_le_bytes());
    let (_, bump) = Pubkey::find_program_address(
        &[b"payment", sender.as_ref(), &NONCE.to_le_bytes()],
        &l.program,
    );
    data.push(bump);
    let forged = Keypair::new().pubkey();
    let lamports = l.svm.minimum_balance_for_rent_exemption(data.len()) + AMOUNT;
    let program = l.program;
    l.svm
        .set_account(forged, Account { lamports, data, owner: program, executable: false, rent_epoch: 0 })
        .unwrap();
    assert!(Lab::logs(&l.claim_on(&scammer, forged, SECRET)).contains("ConstraintSeeds"));
}

#[test]
fn an_account_owned_by_another_program_is_rejected() {
    let mut l = lab();
    let scammer = l.scammer.insecure_clone();
    let fake = Keypair::new().pubkey();
    let data = discriminator("account", "Payment").into_iter().chain([0u8; 121]).collect::<Vec<_>>();
    let lamports = l.svm.minimum_balance_for_rent_exemption(data.len());
    l.svm
        .set_account(fake, Account { lamports, data, owner: Keypair::new().pubkey(), executable: false, rent_epoch: 0 })
        .unwrap();
    assert!(Lab::logs(&l.claim_on(&scammer, fake, SECRET)).contains("AccountOwnedByWrongProgram"));
}
