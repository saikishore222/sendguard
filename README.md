# SendGuard

Crypto's test transfer, done right. Verify once, then send without fear.

Scammers plant lookalike addresses in your wallet history, hoping you copy one. On Solana, as on most chains, a payment to the wrong address is final. SendGuard holds a first payment until the recipient proves they have a secret you shared with them. If nobody can, the money goes back to you.

Live on Solana devnet: [`HVPxF8vnBJXjy7eeX34BzMvAKMFxUdPDuZLHUFTUvPfu`](https://explorer.solana.com/address/HVPxF8vnBJXjy7eeX34BzMvAKMFxUdPDuZLHUFTUvPfu?cluster=devnet)

## How it works

1. **Send.** Your money goes into a hold on Solana, tied to one recipient address and the hash of a random secret.
2. **Share.** You send your friend a link containing the secret, outside the blockchain.
3. **Claim.** The money is released only if the claim is signed by the recipient's wallet and includes the right secret.
4. **Or get it back.** You can cancel any time before it's claimed. After the hold ends, anyone can trigger the refund, and it can only pay you.
5. **Next time.** Once an address has claimed, it's a verified contact and later payments go straight through.

A scammer who owns the lookalike address never gets the secret, so they can't claim. You don't need to notice the mistake for the money to come back.

## Why it needs both checks

| Who tries to claim | Has the wallet | Has the secret | Result |
|---|---|---|---|
| Your friend | Yes | Yes | Paid |
| Scammer at the lookalike address | Yes | No | Rejected |
| Someone who stole the link | No | Yes | Rejected |

The secret is 32 random bytes, so it can't be guessed from its on-chain hash. It becomes public when a claim is submitted, which doesn't help anyone else, because the claim must also be signed by the recipient.

## Run the demo

Requires Python 3 for a static server. No wallet install is needed.

```bash
python3 -m http.server 4180 --directory web
```

Open <http://localhost:4180>. The page shows three working wallets: You, Friend and Scammer. Every action sends a real devnet transaction, listed with an Explorer link. The demo's two lookalike addresses are real keypairs that match on their first and last 4 characters, found with a birthday search over about 9.5M keys.

The "Try it yourself" section connects Phantom, Solflare or Backpack on devnet, so you can send to any address and share a working claim link. To host it, deploy the `web/` folder to any static host. It must be served over HTTPS for the browser's crypto API to work.

## Run the tests

```bash
cargo-build-sbf --tools-version v1.52
```

```bash
cd tests-host && cargo test
```

The 15 tests run the compiled Anchor program on LiteSVM. They cover the claim, wrong secret, stolen secret, expired hold, cancel, permissionless refund, wrong sender, double payout, prefunded payment address, reused nonce, bad inputs and forged payment accounts.

## IDL

The IDL is in `idl/sendguard.json` and published on devnet. The site and the tests read instruction and error codes from it. After changing the program, regenerate it with Anchor CLI 1.2.2:

```bash
make idl
```

## Limits

- **Devnet prototype.** It hasn't been audited, and it shouldn't hold real money.
- **Not protection from someone you trust.** If the scammer is the person you're talking to, you'd hand them the secret.
- **Exchange deposit addresses can't claim.** They'd need a timed hold instead, which isn't built.
- **The refund needs a transaction.** Solana programs don't run on a timer, so something has to submit the refund after the hold ends. Anyone can, and the demo page does it for you.
- **Demo wallets are public on purpose.** `web/demo-config.js` contains devnet-only keys so visitors don't need a wallet. Anyone can spend that test SOL, so the demo can run dry. Never reuse these keys.
- **Verified contacts live in the browser.** They aren't stored on-chain in this version.
- **The lookalike warning is a hint.** It compares the first 4 and last 4 characters against addresses this browser already knows. The secret is what protects you.
- **Sponsored claim fees are demo-only.** The page pays the network fee for a claim from the public demo wallet, so a brand-new wallet can claim. A real product would need its own way to cover this.
- **Only SOL.** SPL tokens aren't supported yet.

## What's next

- Wallet integration, so the check appears when you paste a new address
- SPL tokens, starting with USDC
- Guardian: a trusted person can cancel a large transfer during the hold
- Timed holds for exchange deposit addresses

## History

The version submitted to the hackathon was a native Solana program, still on devnet at `5BtMWAqt4fRJM21kXepCRayNwRp3Nz8ic2nMeshLjEgN`. Its code is in the first commit, and its on-chain logs use the working name `handshake`. The live site now uses this Anchor version.

## Layout

```
programs/     The Anchor program
tests-host/   LiteSVM tests against the compiled program
idl/          The program IDL
web/          The demo page
```
