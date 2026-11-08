use litesvm::LiteSVM;
use litesvm::types::TransactionResult;
use sha2::{Digest, Sha256};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

const AMOUNT: u64 = 500_000_000;
const SECRET: [u8; 32] = [7; 32];
const HOLD: i64 = 60;

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
    let program = Keypair::new().pubkey();
    let elf = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../program/target/deploy/handshake.so"
    ))
    .expect("build the program first");
    svm.add_program(program, &elf).unwrap();
    let (sender, friend, scammer) = (Keypair::new(), Keypair::new(), Keypair::new());
    for who in [&sender, &friend, &scammer] {
        svm.airdrop(&who.pubkey(), 5_000_000_000).unwrap();
    }
    let (payment, _) = Pubkey::find_program_address(
        &[b"payment", sender.pubkey().as_ref(), &1u64.to_le_bytes()],
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

    fn send_to(&mut self, recipient: Pubkey) -> TransactionResult {
        let mut data = vec![0u8];
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(&AMOUNT.to_le_bytes());
        data.extend_from_slice(&HOLD.to_le_bytes());
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

    fn claim(&mut self, who: &Keypair, secret: [u8; 32]) -> TransactionResult {
        let mut data = vec![1u8];
        data.extend_from_slice(&secret);
        let ix = Instruction::new_with_bytes(
            self.program,
            &data,
            vec![
                AccountMeta::new(who.pubkey(), true),
                AccountMeta::new(self.payment, false),
                AccountMeta::new(self.sender.pubkey(), false),
            ],
        );
        self.submit(ix, who)
    }

    fn reclaim(&mut self, who: &Keypair) -> TransactionResult {
        let ix = Instruction::new_with_bytes(
            self.program,
            &[2u8],
            vec![
                AccountMeta::new(who.pubkey(), true),
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
fn scammer_who_owns_the_address_cannot_claim_without_the_secret() {
    let mut l = lab();
    let scammer = l.scammer.insecure_clone();
    l.send_to(scammer.pubkey()).unwrap();
    let held = l.balance(&l.payment);
    let result = l.claim(&scammer, [9; 32]);
    assert!(Lab::logs(&result).contains("wrong secret"));
    assert_eq!(l.balance(&l.payment), held);
}

#[test]
fn stolen_secret_is_useless_from_another_wallet() {
    let mut l = lab();
    let (friend, scammer) = (l.friend.insecure_clone(), l.scammer.insecure_clone());
    l.send_to(friend.pubkey()).unwrap();
    let result = l.claim(&scammer, SECRET);
    assert!(Lab::logs(&result).contains("signed by the payment's recipient"));
}

#[test]
fn nobody_can_claim_after_the_hold_ends() {
    let mut l = lab();
    let friend = l.friend.insecure_clone();
    l.send_to(friend.pubkey()).unwrap();
    l.pass_deadline();
    assert!(Lab::logs(&l.claim(&friend, SECRET)).contains("hold has expired"));
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
    assert!(Lab::logs(&l.reclaim(&scammer)).contains("only the sender can cancel"));
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
    let mut data = vec![1u8];
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
    let friend = l.friend.insecure_clone();
    l.send_to(friend.pubkey()).unwrap();
    l.claim(&friend, SECRET).unwrap();
    assert!(l.claim(&friend, SECRET).is_err());
    let sender = l.sender.insecure_clone();
    assert!(l.reclaim(&sender).is_err());
}
