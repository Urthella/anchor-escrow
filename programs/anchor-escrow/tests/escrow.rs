//! Runs the compiled program in LiteSVM. Build it first: `cargo build-sbf`.
use anchor_escrow::{accounts, instruction, Escrow, ID as PROGRAM_ID};
use anchor_lang::{prelude::Pubkey, AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id as ata;
use litesvm::LiteSVM;
use litesvm_token::{
    get_spl_account, spl_token::state::Account as TokenAccount, CreateAssociatedTokenAccount,
    CreateMint, MintTo,
};
use solana_clock::Clock;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

const TOKEN: Pubkey = anchor_spl::token::ID;
const TOKEN_2022: Pubkey = anchor_spl::token_2022::ID;

/// A maker holding 1_000 of mint A and a taker holding 500 of mint B.
struct World {
    svm: LiteSVM,
    maker: Keypair,
    taker: Keypair,
    mint_a: Pubkey,
    mint_b: Pubkey,
    token_program: Pubkey,
}

impl World {
    fn new(token_program: Pubkey) -> Self {
        let mut svm = LiteSVM::new();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/deploy/anchor_escrow.so"
        );
        let program = std::fs::read(path).expect("build the program first: cargo build-sbf");
        svm.add_program(PROGRAM_ID, &program).unwrap();

        let (maker, taker) = (Keypair::new(), Keypair::new());
        for who in [&maker, &taker] {
            svm.airdrop(&who.pubkey(), 10_000_000_000).unwrap();
        }
        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .token_program_id(&token_program)
            .send()
            .unwrap();
        let mint_b = CreateMint::new(&mut svm, &taker)
            .decimals(9)
            .token_program_id(&token_program)
            .send()
            .unwrap();
        let maker_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .token_program_id(&token_program)
            .send()
            .unwrap();
        let taker_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .token_program_id(&token_program)
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_a, 1_000)
            .token_program_id(&token_program)
            .send()
            .unwrap();
        MintTo::new(&mut svm, &taker, &mint_b, &taker_b, 500)
            .token_program_id(&token_program)
            .send()
            .unwrap();
        World {
            svm,
            maker,
            taker,
            mint_a,
            mint_b,
            token_program,
        }
    }

    fn escrow(&self, seed: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[b"escrow", self.maker.pubkey().as_ref(), &seed.to_le_bytes()],
            &PROGRAM_ID,
        )
        .0
    }

    fn ata(&self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        ata(owner, mint, &self.token_program)
    }

    fn balance(&self, owner: &Pubkey, mint: &Pubkey) -> u64 {
        get_spl_account::<TokenAccount>(&self.svm, &self.ata(owner, mint))
            .map(|a| a.amount)
            .unwrap_or(0)
    }

    fn exists(&self, address: &Pubkey) -> bool {
        self.svm
            .get_account(address)
            .is_some_and(|a| a.lamports > 0)
    }

    fn send(
        &mut self,
        data: Vec<u8>,
        accounts: Vec<anchor_lang::prelude::AccountMeta>,
        signer: &Keypair,
    ) -> Result<(), String> {
        self.svm.expire_blockhash(); // identical retries must not be deduplicated as AlreadyProcessed
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts,
            data,
        };
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&signer.pubkey()),
            &[signer],
            self.svm.latest_blockhash(),
        );
        self.svm
            .send_transaction(tx)
            .map(|_| ())
            .map_err(|e| format!("{:?} {:?}", e.err, e.meta.logs))
    }

    fn make(&mut self, seed: u64, deposit: u64, receive: u64) -> Result<(), String> {
        self.make_with(seed, deposit, receive, self.mint_b, 0)
    }

    fn make_with(
        &mut self,
        seed: u64,
        deposit: u64,
        receive: u64,
        mint_b: Pubkey,
        expires_at: i64,
    ) -> Result<(), String> {
        let (maker, escrow) = (self.maker.pubkey(), self.escrow(seed));
        let metas = accounts::Make {
            maker,
            mint_a: self.mint_a,
            mint_b,
            maker_ata_a: self.ata(&maker, &self.mint_a),
            escrow,
            vault: self.ata(&escrow, &self.mint_a),
            associated_token_program: anchor_spl::associated_token::ID,
            token_program: self.token_program,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None);
        let maker_kp = self.maker.insecure_clone();
        self.send(
            instruction::Make {
                seed,
                deposit,
                receive,
                expires_at,
            }
            .data(),
            metas,
            &maker_kp,
        )
    }

    fn set_time(&mut self, unix_timestamp: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = unix_timestamp;
        self.svm.set_sysvar(&clock);
    }

    fn take(&mut self, seed: u64, mint_b: Pubkey) -> Result<(), String> {
        let (maker, taker, escrow) = (self.maker.pubkey(), self.taker.pubkey(), self.escrow(seed));
        let metas = accounts::Take {
            taker,
            maker,
            mint_a: self.mint_a,
            mint_b,
            taker_ata_a: self.ata(&taker, &self.mint_a),
            taker_ata_b: self.ata(&taker, &mint_b),
            maker_ata_b: self.ata(&maker, &mint_b),
            escrow,
            vault: self.ata(&escrow, &self.mint_a),
            associated_token_program: anchor_spl::associated_token::ID,
            token_program: self.token_program,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None);
        let taker_kp = self.taker.insecure_clone();
        self.send(instruction::Take {}.data(), metas, &taker_kp)
    }

    /// `as_who` signs in the maker's slot: the real maker, or someone trying to impersonate them.
    fn refund(&mut self, seed: u64, as_who: &Keypair) -> Result<(), String> {
        let escrow = self.escrow(seed);
        let who = as_who.pubkey();
        let metas = accounts::Refund {
            maker: who,
            mint_a: self.mint_a,
            maker_ata_a: self.ata(&who, &self.mint_a),
            escrow,
            vault: self.ata(&escrow, &self.mint_a),
            associated_token_program: anchor_spl::associated_token::ID,
            token_program: self.token_program,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None);
        self.send(instruction::Refund {}.data(), metas, as_who)
    }
}

fn swap(token_program: Pubkey) {
    let mut w = World::new(token_program);
    let (maker, taker, escrow) = (w.maker.pubkey(), w.taker.pubkey(), w.escrow(7));

    w.make(7, 1_000, 500).unwrap();
    assert_eq!(w.balance(&maker, &w.mint_a), 0);
    assert_eq!(
        w.balance(&escrow, &w.mint_a),
        1_000,
        "vault holds the deposit"
    );
    let state =
        Escrow::try_deserialize(&mut &w.svm.get_account(&escrow).unwrap().data[..]).unwrap();
    assert_eq!(
        (
            state.seed,
            state.maker,
            state.mint_a,
            state.mint_b,
            state.receive
        ),
        (7, maker, w.mint_a, w.mint_b, 500)
    );

    let maker_lamports = w.svm.get_balance(&maker).unwrap();
    w.take(7, w.mint_b).unwrap();
    assert_eq!(
        w.balance(&taker, &w.mint_a),
        1_000,
        "taker got the whole vault"
    );
    assert_eq!(w.balance(&maker, &w.mint_b), 500, "maker got paid");
    assert_eq!(w.balance(&taker, &w.mint_b), 0);
    assert!(
        !w.exists(&escrow) && !w.exists(&w.ata(&escrow, &w.mint_a)),
        "escrow and vault closed"
    );
    assert!(
        w.svm.get_balance(&maker).unwrap() > maker_lamports,
        "rent went back to the maker"
    );
}

#[test]
fn make_then_take_swaps_and_closes_everything() {
    swap(TOKEN);
}

#[test]
fn works_with_token_2022_mints() {
    swap(TOKEN_2022);
}

#[test]
fn refund_returns_the_deposit_and_only_the_maker_can_refund() {
    let mut w = World::new(TOKEN);
    w.make(1, 1_000, 500).unwrap();
    let taker = w.taker.insecure_clone();
    assert!(
        w.refund(1, &taker).is_err(),
        "the taker signing as maker must fail the escrow's seeds and has_one checks"
    );

    let maker = w.maker.insecure_clone();
    w.refund(1, &maker).unwrap();
    assert_eq!(w.balance(&maker.pubkey(), &w.mint_a), 1_000);
    assert!(!w.exists(&w.escrow(1)));
    assert!(
        w.take(1, w.mint_b).is_err(),
        "nothing left to take after a refund"
    );
}

#[test]
fn take_must_pay_in_the_mint_the_maker_asked_for() {
    let mut w = World::new(TOKEN);
    w.make(2, 1_000, 500).unwrap();
    let taker = w.taker.insecure_clone();
    let fake = CreateMint::new(&mut w.svm, &taker)
        .decimals(9)
        .send()
        .unwrap();
    let fake_ata = CreateAssociatedTokenAccount::new(&mut w.svm, &taker, &fake)
        .send()
        .unwrap();
    MintTo::new(&mut w.svm, &taker, &fake, &fake_ata, 500)
        .send()
        .unwrap();

    let err = w.take(2, fake).unwrap_err();
    assert!(
        err.contains("Custom(2001)"),
        "expected Anchor's ConstraintHasOne (2001), got {err}"
    );
    assert_eq!(w.balance(&w.escrow(2), &w.mint_a), 1_000, "vault untouched");
}

#[test]
fn make_rejects_zero_amounts_and_a_single_mint() {
    let mut w = World::new(TOKEN);
    assert!(
        w.make(3, 0, 500).unwrap_err().contains("Custom(6000)"),
        "ZeroAmount"
    );
    assert!(
        w.make(3, 1_000, 0).unwrap_err().contains("Custom(6000)"),
        "ZeroAmount"
    );
    let mint_a = w.mint_a;
    assert!(
        w.make_with(3, 1_000, 500, mint_a, 0)
            .unwrap_err()
            .contains("Custom(6001)"),
        "SameMint"
    );
}

#[test]
fn an_offer_can_be_taken_until_it_expires() {
    let mut w = World::new(TOKEN);
    w.set_time(1_000_000);
    let mint_b = w.mint_b;
    w.make_with(4, 1_000, 500, mint_b, 1_000_100).unwrap();
    w.set_time(1_000_099);
    w.take(4, mint_b).unwrap();
    assert_eq!(w.balance(&w.taker.pubkey(), &w.mint_a), 1_000);
}

#[test]
fn an_expired_offer_cannot_be_taken_but_can_be_refunded() {
    let mut w = World::new(TOKEN);
    w.set_time(1_000_000);
    let mint_b = w.mint_b;
    w.make_with(5, 1_000, 500, mint_b, 1_000_100).unwrap();
    w.set_time(1_000_100); // the deadline itself is already too late
    let err = w.take(5, mint_b).unwrap_err();
    assert!(err.contains("Custom(6002)"), "Expired, got {err}");
    let maker = w.maker.insecure_clone();
    w.refund(5, &maker).unwrap();
    assert_eq!(w.balance(&maker.pubkey(), &w.mint_a), 1_000);
}

#[test]
fn make_rejects_an_expiry_in_the_past() {
    let mut w = World::new(TOKEN);
    w.set_time(1_000_000);
    let mint_b = w.mint_b;
    let err = w.make_with(6, 1_000, 500, mint_b, 1_000_000).unwrap_err();
    assert!(err.contains("Custom(6003)"), "ExpiryInPast, got {err}");
}
