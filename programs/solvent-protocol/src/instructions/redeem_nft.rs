use crate::constants::*;
use crate::errors::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token;
use anchor_spl::token::{Mint, Token, TokenAccount};

// Burn droplets and redeem an NFT from the bucket in exchange
pub fn redeem_nft(ctx: Context<RedeemNft>, swap: bool) -> Result<()> {
    // Not setting the flag cause the existing flag is what it should be
    ctx.accounts.swap_state.bump = *ctx.bumps.get("swap_state").unwrap();
    ctx.accounts.swap_state.droplet_mint = ctx.accounts.droplet_mint.key();
    ctx.accounts.swap_state.signer = ctx.accounts.signer.key();

    let fee_basis_points;

    if swap {
        if ctx.accounts.swap_state.flag {
            // User passed swap=true and he's eligible
            fee_basis_points = SWAP_FEE_BASIS_POINTS;
            ctx.accounts.swap_state.flag = false;
        } else {
            // User passed swap=true but he's not eligible for swap yet
            return err!(SolventError::SwapNotAllowed);
        }
    } else {
        // User passed swap=false
        fee_basis_points = REDEEM_FEE_BASIS_POINTS;

        // Burn droplets from the signer's account
        let burn_droplets_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info().clone(),
            token::Burn {
                mint: ctx.accounts.droplet_mint.to_account_info().clone(),
                from: ctx
                    .accounts
                    .signer_droplet_token_account
                    .to_account_info()
                    .clone(),
                authority: ctx.accounts.signer.to_account_info().clone(),
            },
        );
        token::burn(
            burn_droplets_ctx,
            DROPLETS_PER_NFT as u64 * LAMPORTS_PER_DROPLET,
        )?;
    }

    // Calculate redeem/swap fee
    let total_fee_amount = (DROPLETS_PER_NFT as u64)
        .checked_mul(LAMPORTS_PER_DROPLET as u64)
        .unwrap()
        .checked_mul(fee_basis_points as u64)
        .unwrap()
        .checked_div(10_000_u64)
        .unwrap();

    let distributor_fee_amount = total_fee_amount
        .checked_mul(DISTRIBUTOR_FEE_BASIS_POINTS as u64)
        .unwrap()
        .checked_div(10_000_u64)
        .unwrap();

    let solvent_treasury_fee_amount = total_fee_amount
        .checked_sub(distributor_fee_amount)
        .unwrap();

    // Ensure correct fee calculation
    require!(
        distributor_fee_amount
            .checked_add(solvent_treasury_fee_amount as u64)
            .unwrap()
            == total_fee_amount,
        SolventError::FeeDistributionIncorrect
    );

    // Transfer DISTRIBUTOR_FEE_PERCENTAGE % fee to distributor
    let transfer_fee_distributor_ctx = CpiContext::new(
        ctx.accounts.token_program.to_account_info().clone(),
        token::Transfer {
            from: ctx
                .accounts
                .signer_droplet_token_account
                .to_account_info()
                .clone(),
            to: ctx
                .accounts
                .distributor_droplet_token_account
                .to_account_info()
                .clone(),
            authority: ctx.accounts.signer.to_account_info().clone(),
        },
    );
    token::transfer(transfer_fee_distributor_ctx, distributor_fee_amount)?;

    // Transfer (100 - DISTRIBUTOR_FEE_PERCENTAGE) % fee to solvent treasury
    let transfer_fee_treasury_ctx = CpiContext::new(
        ctx.accounts.token_program.to_account_info().clone(),
        token::Transfer {
            from: ctx
                .accounts
                .signer_droplet_token_account
                .to_account_info()
                .clone(),
            to: ctx
                .accounts
                .solvent_treasury_droplet_token_account
                .to_account_info()
                .clone(),
            authority: ctx.accounts.signer.to_account_info().clone(),
        },
    );
    token::transfer(transfer_fee_treasury_ctx, solvent_treasury_fee_amount)?;

    // Get Solvent authority signer seeds
    let solvent_authority_bump = *ctx.bumps.get("solvent_authority").unwrap();
    let solvent_authority_seeds = &[SOLVENT_AUTHORITY_SEED.as_bytes(), &[solvent_authority_bump]];
    let solvent_authority_signer_seeds = &[&solvent_authority_seeds[..]];

    // Transfer NFT to destination account
    let transfer_nft_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info().clone(),
        token::Transfer {
            from: ctx
                .accounts
                .solvent_nft_token_account
                .to_account_info()
                .clone(),
            to: ctx
                .accounts
                .destination_nft_token_account
                .to_account_info()
                .clone(),
            authority: ctx.accounts.solvent_authority.to_account_info().clone(),
        },
        solvent_authority_signer_seeds,
    );
    token::transfer(transfer_nft_ctx, 1)?;

    // Close bucket's NFT token account to reclaim lamports
    let close_nft_token_account_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info().clone(),
        token::CloseAccount {
            account: ctx
                .accounts
                .solvent_nft_token_account
                .to_account_info()
                .clone(),
            destination: ctx.accounts.signer.to_account_info().clone(),
            authority: ctx.accounts.solvent_authority.to_account_info().clone(),
        },
        solvent_authority_signer_seeds,
    );
    token::close_account(close_nft_token_account_ctx)?;

    // Decrement counter
    ctx.accounts.bucket_state.num_nfts_in_bucket = ctx
        .accounts
        .bucket_state
        .num_nfts_in_bucket
        .checked_sub(1)
        .unwrap();

    // Emit success event
    emit!(RedeemNftEvent {
        droplet_mint: ctx.accounts.droplet_mint.key(),
        nft_mint: ctx.accounts.nft_mint.key(),
        signer: ctx.accounts.signer.key(),
        destination_nft_token_account: ctx.accounts.destination_nft_token_account.key(),
        signer_droplet_token_account: ctx.accounts.signer_droplet_token_account.key(),
        swap,
        num_nfts_in_bucket: ctx.accounts.bucket_state.num_nfts_in_bucket
    });

    Ok(())
}

#[derive(Accounts)]
pub struct RedeemNft<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,

    #[account(
        seeds = [SOLVENT_AUTHORITY_SEED.as_bytes()],
        bump,
    )]
    /// CHECK: Safe because this read-only account only gets used as a constraint
    pub solvent_authority: UncheckedAccount<'info>,

    /// CHECK: Safe because we are using this account just for a constraint
    pub distributor: UncheckedAccount<'info>,

    #[account(
        init_if_needed,
        payer = signer,
        associated_token::mint = droplet_mint,
        associated_token::authority = distributor
    )]
    pub distributor_droplet_token_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [
            droplet_mint.key().as_ref(),
            BUCKET_SEED.as_bytes()
        ],
        bump = bucket_state.bump,
        has_one = droplet_mint
    )]
    pub bucket_state: Box<Account<'info, BucketStateV3>>,

    #[account(
        mut,
        seeds = [
            droplet_mint.key().as_ref(),
            nft_mint.key().as_ref(),
            DEPOSIT_SEED.as_bytes()
        ],
        bump = deposit_state.bump,
        close = signer,
        has_one = nft_mint,
        has_one = droplet_mint
    )]
    pub deposit_state: Box<Account<'info, DepositState>>,

    #[account(
        init_if_needed,
        seeds = [
            droplet_mint.key().as_ref(),
            signer.key().as_ref(),
            SWAP_SEED.as_bytes()
        ],
        bump,
        payer = signer,
        space = SwapState::LEN
    )]
    pub swap_state: Box<Account<'info, SwapState>>,

    #[account(mut)]
    pub droplet_mint: Box<Account<'info, Mint>>,

    pub nft_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        address = get_associated_token_address(solvent_authority.key, &nft_mint.key()),
    )]
    pub solvent_nft_token_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = destination_nft_token_account.mint == nft_mint.key(),
    )]
    pub destination_nft_token_account: Box<Account<'info, TokenAccount>>,

    #[account(address = SOLVENT_CORE_TREASURY @ SolventError::SolventTreasuryInvalid)]
    /// CHECK: Safe because there are enough constraints set
    pub solvent_treasury: UncheckedAccount<'info>,

    #[account(
        init_if_needed,
        payer = signer,
        associated_token::mint = droplet_mint,
        associated_token::authority = solvent_treasury,
    )]
    pub solvent_treasury_droplet_token_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = signer_droplet_token_account.mint == droplet_mint.key()
    )]
    pub signer_droplet_token_account: Box<Account<'info, TokenAccount>>,

    // Solana ecosystem program addresses
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

#[event]
pub struct RedeemNftEvent {
    pub signer: Pubkey,
    pub nft_mint: Pubkey,
    pub droplet_mint: Pubkey,
    pub signer_droplet_token_account: Pubkey,
    pub destination_nft_token_account: Pubkey,
    pub swap: bool,
    // Counters
    pub num_nfts_in_bucket: u16,
}
