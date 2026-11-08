use solana_program::{
    account_info::AccountInfo,
    clock::Clock,
    entrypoint,
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};
use solana_sha256_hasher::hash;
use solana_system_interface::instruction as system_instruction;

pub const PAYMENT_SEED: &[u8] = b"payment";
pub const PAYMENT_LEN: usize = 113;
pub const MAX_HOLD_SECONDS: i64 = 30 * 24 * 60 * 60;

const TAG: u8 = 1;
const SENDER: usize = 1;
const RECIPIENT: usize = 33;
const SECRET_HASH: usize = 65;
const AMOUNT: usize = 97;
const DEADLINE: usize = 105;

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let (instruction, args) = data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match instruction {
        0 => send(program_id, accounts, args),
        1 => claim(program_id, accounts, args),
        2 => reclaim(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn send(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    let [sender, payment, recipient, system_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if args.len() != 56 {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !sender.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_program.key != solana_system_interface::program::id() {
        return Err(ProgramError::IncorrectProgramId);
    }
    let nonce = &args[0..8];
    let amount = u64::from_le_bytes(args[8..16].try_into().unwrap());
    let hold_seconds = i64::from_le_bytes(args[16..24].try_into().unwrap());
    let secret_hash = &args[24..56];
    if amount == 0 || !(1..=MAX_HOLD_SECONDS).contains(&hold_seconds) {
        msg!("handshake: invalid amount or hold time");
        return Err(ProgramError::InvalidArgument);
    }
    let (expected, bump) = Pubkey::find_program_address(
        &[PAYMENT_SEED, sender.key.as_ref(), nonce],
        program_id,
    );
    if *payment.key != expected {
        return Err(ProgramError::InvalidSeeds);
    }
    if *payment.owner != solana_system_interface::program::id() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    let rent = Rent::get()?.minimum_balance(PAYMENT_LEN);
    let needed = rent
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?
        .saturating_sub(payment.lamports());
    if needed > 0 {
        invoke(
            &system_instruction::transfer(sender.key, payment.key, needed),
            &[sender.clone(), payment.clone(), system_program.clone()],
        )?;
    }
    let seeds: &[&[u8]] = &[PAYMENT_SEED, sender.key.as_ref(), nonce, &[bump]];
    invoke_signed(
        &system_instruction::allocate(payment.key, PAYMENT_LEN as u64),
        &[payment.clone(), system_program.clone()],
        &[seeds],
    )?;
    invoke_signed(
        &system_instruction::assign(payment.key, program_id),
        &[payment.clone(), system_program.clone()],
        &[seeds],
    )?;

    let deadline = Clock::get()?
        .unix_timestamp
        .checked_add(hold_seconds)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    let mut state = payment.try_borrow_mut_data()?;
    state[0] = TAG;
    state[SENDER..RECIPIENT].copy_from_slice(sender.key.as_ref());
    state[RECIPIENT..SECRET_HASH].copy_from_slice(recipient.key.as_ref());
    state[SECRET_HASH..AMOUNT].copy_from_slice(secret_hash);
    state[AMOUNT..DEADLINE].copy_from_slice(&amount.to_le_bytes());
    state[DEADLINE..PAYMENT_LEN].copy_from_slice(&deadline.to_le_bytes());
    msg!("handshake: {} lamports held until {}", amount, deadline);
    Ok(())
}

struct Payment {
    sender: Pubkey,
    recipient: Pubkey,
    secret_hash: [u8; 32],
    amount: u64,
    deadline: i64,
}

fn load(program_id: &Pubkey, payment: &AccountInfo) -> Result<Payment, ProgramError> {
    if payment.owner != program_id {
        msg!("handshake: payment is closed or does not exist");
        return Err(ProgramError::IllegalOwner);
    }
    let state = payment.try_borrow_data()?;
    if state.len() != PAYMENT_LEN || state[0] != TAG {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(Payment {
        sender: Pubkey::new_from_array(state[SENDER..RECIPIENT].try_into().unwrap()),
        recipient: Pubkey::new_from_array(state[RECIPIENT..SECRET_HASH].try_into().unwrap()),
        secret_hash: state[SECRET_HASH..AMOUNT].try_into().unwrap(),
        amount: u64::from_le_bytes(state[AMOUNT..DEADLINE].try_into().unwrap()),
        deadline: i64::from_le_bytes(state[DEADLINE..PAYMENT_LEN].try_into().unwrap()),
    })
}

fn close(payment: &AccountInfo) -> ProgramResult {
    **payment.try_borrow_mut_lamports()? = 0;
    payment.try_borrow_mut_data()?.fill(0);
    Ok(())
}

fn claim(program_id: &Pubkey, accounts: &[AccountInfo], args: &[u8]) -> ProgramResult {
    let [recipient, payment, sender] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let state = load(program_id, payment)?;
    if *recipient.key != state.recipient || !recipient.is_signer {
        msg!("handshake: claim must be signed by the payment's recipient");
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *sender.key != state.sender {
        return Err(ProgramError::InvalidAccountData);
    }
    if Clock::get()?.unix_timestamp > state.deadline {
        msg!("handshake: hold has expired, this payment can only be refunded");
        return Err(ProgramError::InvalidArgument);
    }
    if args.len() != 32 || hash(args).to_bytes() != state.secret_hash {
        msg!("handshake: wrong secret");
        return Err(ProgramError::InvalidArgument);
    }

    let total = payment.lamports();
    let rent_back = total
        .checked_sub(state.amount)
        .ok_or(ProgramError::InsufficientFunds)?;
    **recipient.try_borrow_mut_lamports()? = recipient
        .lamports()
        .checked_add(state.amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    **sender.try_borrow_mut_lamports()? = sender
        .lamports()
        .checked_add(rent_back)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    close(payment)?;
    msg!("handshake: claimed {} lamports", state.amount);
    Ok(())
}

fn reclaim(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let [caller, payment, sender] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let state = load(program_id, payment)?;
    if !caller.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *sender.key != state.sender {
        return Err(ProgramError::InvalidAccountData);
    }
    if *caller.key != state.sender && Clock::get()?.unix_timestamp <= state.deadline {
        msg!("handshake: only the sender can cancel before the hold ends");
        return Err(ProgramError::InvalidArgument);
    }

    let total = payment.lamports();
    **sender.try_borrow_mut_lamports()? = sender
        .lamports()
        .checked_add(total)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    close(payment)?;
    msg!("handshake: {} lamports returned to sender", state.amount);
    Ok(())
}
