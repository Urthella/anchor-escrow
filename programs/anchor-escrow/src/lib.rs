//! A token escrow: the maker locks some of token A and names a price in token B; anyone who pays
//! that price gets the locked tokens, and until then the maker can take them back.
use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{
        close_account, transfer_checked, CloseAccount, Mint, TokenAccount, TokenInterface,
        TransferChecked,
    },
};

declare_id!("8EpkgGzLp7Yv5KKavm7t9gH21eXjg3pkUduZ2Be3pxuX");

#[program]
pub mod anchor_escrow {
    use super::*;

    /// Locks `deposit` of mint A in a vault owned by the escrow and asks `receive` of mint B for it.
    /// `expires_at` is a unix timestamp after which the offer can't be taken; 0 means never.
    pub fn make(
        ctx: Context<Make>,
        seed: u64,
        deposit: u64,
        receive: u64,
        expires_at: i64,
    ) -> Result<()> {
        require!(deposit > 0 && receive > 0, EscrowError::ZeroAmount);
        require_keys_neq!(
            ctx.accounts.mint_a.key(),
            ctx.accounts.mint_b.key(),
            EscrowError::SameMint
        );
        require!(
            expires_at == 0 || expires_at > Clock::get()?.unix_timestamp,
            EscrowError::ExpiryInPast
        );
        ctx.accounts.escrow.set_inner(Escrow {
            seed,
            maker: ctx.accounts.maker.key(),
            mint_a: ctx.accounts.mint_a.key(),
            mint_b: ctx.accounts.mint_b.key(),
            receive,
            expires_at,
            bump: ctx.bumps.escrow,
        });
        let a = &ctx.accounts;
        transfer_checked(
            CpiContext::new(
                a.token_program.key(),
                TransferChecked {
                    from: a.maker_ata_a.to_account_info(),
                    mint: a.mint_a.to_account_info(),
                    to: a.vault.to_account_info(),
                    authority: a.maker.to_account_info(),
                },
            ),
            deposit,
            a.mint_a.decimals,
        )
    }

    /// Pays the maker `receive` of mint B, hands the whole vault to the taker, and closes the escrow.
    pub fn take(ctx: Context<Take>) -> Result<()> {
        let a = &ctx.accounts;
        // An expired offer stays refundable; it just can't be taken any more.
        require!(
            a.escrow.expires_at == 0 || Clock::get()?.unix_timestamp < a.escrow.expires_at,
            EscrowError::Expired
        );
        transfer_checked(
            CpiContext::new(
                a.token_program.key(),
                TransferChecked {
                    from: a.taker_ata_b.to_account_info(),
                    mint: a.mint_b.to_account_info(),
                    to: a.maker_ata_b.to_account_info(),
                    authority: a.taker.to_account_info(),
                },
            ),
            a.escrow.receive,
            a.mint_b.decimals,
        )?;
        release(
            &a.escrow,
            &a.vault,
            &a.mint_a,
            &a.taker_ata_a,
            &a.maker.to_account_info(),
            &a.token_program,
        )
    }

    /// Returns the deposit to the maker and closes the escrow. Only the maker can call it.
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        let a = &ctx.accounts;
        release(
            &a.escrow,
            &a.vault,
            &a.mint_a,
            &a.maker_ata_a,
            &a.maker.to_account_info(),
            &a.token_program,
        )
    }
}

/// Moves everything in the vault to `to`, signing as the escrow PDA, then closes the vault and
/// sends its rent to the maker. The escrow account itself is closed by its `close = maker` constraint.
fn release<'info>(
    escrow: &Account<'info, Escrow>,
    vault: &InterfaceAccount<'info, TokenAccount>,
    mint_a: &InterfaceAccount<'info, Mint>,
    to: &InterfaceAccount<'info, TokenAccount>,
    maker: &AccountInfo<'info>,
    token_program: &Interface<'info, TokenInterface>,
) -> Result<()> {
    let seed = escrow.seed.to_le_bytes();
    let signer: &[&[&[u8]]] = &[&[b"escrow", escrow.maker.as_ref(), &seed, &[escrow.bump]]];
    transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: vault.to_account_info(),
                mint: mint_a.to_account_info(),
                to: to.to_account_info(),
                authority: escrow.to_account_info(),
            },
            signer,
        ),
        vault.amount,
        mint_a.decimals,
    )?;
    close_account(CpiContext::new_with_signer(
        token_program.key(),
        CloseAccount {
            account: vault.to_account_info(),
            destination: maker.clone(),
            authority: escrow.to_account_info(),
        },
        signer,
    ))
}

#[account]
#[derive(InitSpace)]
pub struct Escrow {
    pub seed: u64,
    pub maker: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub receive: u64,
    pub expires_at: i64,
    pub bump: u8,
}

#[error_code]
pub enum EscrowError {
    #[msg("deposit and receive must both be above zero")]
    ZeroAmount,
    #[msg("the escrow must trade two different mints")]
    SameMint,
    #[msg("this offer has expired")]
    Expired,
    #[msg("expires_at must be in the future, or 0 for never")]
    ExpiryInPast,
}

#[derive(Accounts)]
#[instruction(seed: u64)]
pub struct Make<'info> {
    #[account(mut)]
    pub maker: Signer<'info>,
    #[account(mint::token_program = token_program)]
    pub mint_a: InterfaceAccount<'info, Mint>,
    #[account(mint::token_program = token_program)]
    pub mint_b: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = maker,
        associated_token::token_program = token_program,
    )]
    pub maker_ata_a: InterfaceAccount<'info, TokenAccount>,
    #[account(
        init,
        payer = maker,
        space = 8 + Escrow::INIT_SPACE,
        seeds = [b"escrow", maker.key().as_ref(), &seed.to_le_bytes()],
        bump,
    )]
    pub escrow: Account<'info, Escrow>,
    #[account(
        init,
        payer = maker,
        associated_token::mint = mint_a,
        associated_token::authority = escrow,
        associated_token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Take<'info> {
    #[account(mut)]
    pub taker: Signer<'info>,
    /// CHECK: only receives lamports and tokens; `has_one = maker` ties it to the escrow.
    #[account(mut)]
    pub maker: UncheckedAccount<'info>,
    pub mint_a: Box<InterfaceAccount<'info, Mint>>,
    pub mint_b: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init_if_needed,
        payer = taker,
        associated_token::mint = mint_a,
        associated_token::authority = taker,
        associated_token::token_program = token_program,
    )]
    pub taker_ata_a: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint_b,
        associated_token::authority = taker,
        associated_token::token_program = token_program,
    )]
    pub taker_ata_b: Box<InterfaceAccount<'info, TokenAccount>>,
    // The maker is paid into their own associated account, so a taker can't redirect the payment.
    #[account(
        init_if_needed,
        payer = taker,
        associated_token::mint = mint_b,
        associated_token::authority = maker,
        associated_token::token_program = token_program,
    )]
    pub maker_ata_b: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        close = maker,
        has_one = maker,
        has_one = mint_a,
        has_one = mint_b,
        seeds = [b"escrow", maker.key().as_ref(), &escrow.seed.to_le_bytes()],
        bump = escrow.bump,
    )]
    pub escrow: Box<Account<'info, Escrow>>,
    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = escrow,
        associated_token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Refund<'info> {
    #[account(mut)]
    pub maker: Signer<'info>,
    pub mint_a: InterfaceAccount<'info, Mint>,
    #[account(
        init_if_needed,
        payer = maker,
        associated_token::mint = mint_a,
        associated_token::authority = maker,
        associated_token::token_program = token_program,
    )]
    pub maker_ata_a: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        close = maker,
        has_one = maker,
        has_one = mint_a,
        seeds = [b"escrow", maker.key().as_ref(), &escrow.seed.to_le_bytes()],
        bump = escrow.bump,
    )]
    pub escrow: Account<'info, Escrow>,
    #[account(
        mut,
        associated_token::mint = mint_a,
        associated_token::authority = escrow,
        associated_token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
