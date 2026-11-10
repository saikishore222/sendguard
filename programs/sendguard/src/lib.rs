use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Transfer};
use solana_sha256_hasher::hash;

declare_id!("HVPxF8vnBJXjy7eeX34BzMvAKMFxUdPDuZLHUFTUvPfu");

pub const MAX_HOLD_SECONDS: i64 = 30 * 24 * 60 * 60;

#[program]
pub mod sendguard {
    use super::*;

    pub fn send(
        ctx: Context<SendPayment>,
        nonce: u64,
        amount: u64,
        hold_seconds: i64,
        secret_hash: [u8; 32],
    ) -> Result<()> {
        require!(amount > 0, SendGuardError::InvalidAmount);
        require!(
            (1..=MAX_HOLD_SECONDS).contains(&hold_seconds),
            SendGuardError::InvalidHold
        );

        let deadline = Clock::get()?
            .unix_timestamp
            .checked_add(hold_seconds)
            .ok_or(SendGuardError::InvalidHold)?;
        ctx.accounts.payment.set_inner(Payment {
            sender: ctx.accounts.sender.key(),
            recipient: ctx.accounts.recipient.key(),
            secret_hash,
            amount,
            deadline,
            nonce,
            bump: ctx.bumps.payment,
        });

        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                Transfer {
                    from: ctx.accounts.sender.to_account_info(),
                    to: ctx.accounts.payment.to_account_info(),
                },
            ),
            amount,
        )
    }

    pub fn claim(ctx: Context<ClaimPayment>, secret: [u8; 32]) -> Result<()> {
        let payment = &ctx.accounts.payment;
        require!(
            Clock::get()?.unix_timestamp <= payment.deadline,
            SendGuardError::HoldExpired
        );
        require!(
            hash(&secret).to_bytes() == payment.secret_hash,
            SendGuardError::WrongSecret
        );

        let amount = payment.amount;
        ctx.accounts.payment.sub_lamports(amount)?;
        ctx.accounts.recipient.add_lamports(amount)?;
        Ok(())
    }

    pub fn reclaim(ctx: Context<ReclaimPayment>) -> Result<()> {
        let payment = &ctx.accounts.payment;
        require!(
            ctx.accounts.caller.key() == payment.sender
                || Clock::get()?.unix_timestamp > payment.deadline,
            SendGuardError::HoldStillActive
        );
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct SendPayment<'info> {
    #[account(mut)]
    pub sender: Signer<'info>,
    #[account(
        init,
        payer = sender,
        space = 8 + Payment::INIT_SPACE,
        seeds = [b"payment", sender.key().as_ref(), &nonce.to_le_bytes()],
        bump
    )]
    pub payment: Account<'info, Payment>,
    /// CHECK: any address can be paid. Only this address can sign the claim.
    pub recipient: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ClaimPayment<'info> {
    #[account(mut)]
    pub recipient: Signer<'info>,
    #[account(
        mut,
        close = sender,
        has_one = recipient @ SendGuardError::NotRecipient,
        has_one = sender,
        seeds = [b"payment", payment.sender.as_ref(), &payment.nonce.to_le_bytes()],
        bump = payment.bump
    )]
    pub payment: Account<'info, Payment>,
    #[account(mut)]
    pub sender: SystemAccount<'info>,
}

#[derive(Accounts)]
pub struct ReclaimPayment<'info> {
    pub caller: Signer<'info>,
    #[account(
        mut,
        close = sender,
        has_one = sender,
        seeds = [b"payment", payment.sender.as_ref(), &payment.nonce.to_le_bytes()],
        bump = payment.bump
    )]
    pub payment: Account<'info, Payment>,
    #[account(mut)]
    pub sender: SystemAccount<'info>,
}

#[account]
#[derive(InitSpace)]
pub struct Payment {
    pub sender: Pubkey,
    pub recipient: Pubkey,
    pub secret_hash: [u8; 32],
    pub amount: u64,
    pub deadline: i64,
    pub nonce: u64,
    pub bump: u8,
}

#[error_code]
pub enum SendGuardError {
    #[msg("amount must be more than zero")]
    InvalidAmount,
    #[msg("hold must be between 1 second and 30 days")]
    InvalidHold,
    #[msg("claim must be signed by the payment's recipient")]
    NotRecipient,
    #[msg("hold has expired, this payment can only be refunded")]
    HoldExpired,
    #[msg("wrong secret")]
    WrongSecret,
    #[msg("only the sender can cancel before the hold ends")]
    HoldStillActive,
}
