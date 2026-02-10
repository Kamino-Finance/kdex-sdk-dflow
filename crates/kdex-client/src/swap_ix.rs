use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;

mod sysvar_instructions {
    solana_pubkey::declare_id!("Sysvar1nstructions1111111111111111111111111");
}

/// Build the account metas for a KDEX swap instruction.
///
/// All pubkeys must be fully resolved before calling this function.
/// For optional accounts (host fees, scope price feed), pass the program ID
/// as a placeholder to indicate "None" in Anchor's optional account convention.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_account_metas(
    token_transfer_authority: Pubkey,
    pool_key: Pubkey,
    swap_curve: Pubkey,
    pool_authority: Pubkey,
    source_mint: Pubkey,
    destination_mint: Pubkey,
    source_vault: Pubkey,
    destination_vault: Pubkey,
    source_fees_vault: Pubkey,
    source_token_account: Pubkey,
    destination_token_account: Pubkey,
    source_token_program: Pubkey,
    destination_token_program: Pubkey,
    source_token_host_fees_account: Pubkey,
    scope_price_feed: Pubkey,
) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(token_transfer_authority, true),
        AccountMeta::new(pool_key, false),
        AccountMeta::new_readonly(swap_curve, false),
        AccountMeta::new_readonly(pool_authority, false),
        AccountMeta::new_readonly(source_mint, false),
        AccountMeta::new_readonly(destination_mint, false),
        AccountMeta::new(source_vault, false),
        AccountMeta::new(destination_vault, false),
        AccountMeta::new(source_fees_vault, false),
        AccountMeta::new(source_token_account, false),
        AccountMeta::new(destination_token_account, false),
        AccountMeta::new_readonly(source_token_program, false),
        AccountMeta::new_readonly(destination_token_program, false),
        // instructions sysvar
        AccountMeta::new_readonly(sysvar_instructions::ID, false),
        // source_token_host_fees_account - passing program_id means None
        AccountMeta::new(source_token_host_fees_account, false),
        // scope_price_feed - real address for oracle curves, program_id (None) for others
        AccountMeta::new_readonly(scope_price_feed, false),
    ]
}
