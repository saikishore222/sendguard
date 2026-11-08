const { Connection, Keypair, PublicKey, SystemProgram, Transaction, TransactionInstruction, LAMPORTS_PER_SOL } = solanaWeb3;
const cfg = window.HANDSHAKE_CONFIG;
const connection = new Connection(cfg.rpc, "confirmed");
const programId = new PublicKey(cfg.programId);

const DEMO_AMOUNT = 0.01;
const DEMO_HOLD = 40;
const HISTORY_AMOUNT = 0.001 * LAMPORTS_PER_SOL;
const MIN_AMOUNT = 0.002;
const FEE_FLOOR = 0.002 * LAMPORTS_PER_SOL;
const YOU_MINIMUM = 0.03 * LAMPORTS_PER_SOL;
const YOU_TOP_UP = 0.05 * LAMPORTS_PER_SOL;
const GAS_TOP_UP = 0.005 * LAMPORTS_PER_SOL;
const RECYCLE_ABOVE = 0.04 * LAMPORTS_PER_SOL;
const RECYCLE_KEEP = 0.01 * LAMPORTS_PER_SOL;

const keypair = bytes => Keypair.fromSecretKey(Uint8Array.from(bytes));
const funder = keypair(cfg.funder);
const friend = keypair(cfg.friend);
const scammer = keypair(cfg.scammer);
const you = loadYou();

function read(key, fallback) {
  try {
    const value = localStorage.getItem(key);
    return value ? JSON.parse(value) : fallback;
  } catch {
    return fallback;
  }
}

function write(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch {}
}

function loadYou() {
  const saved = read("handshake-you", null);
  if (saved) return keypair(saved);
  const created = Keypair.generate();
  write("handshake-you", Array.from(created.secretKey));
  return created;
}

function el(tag, props, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node[key] = value;
    else if (key === "disabled" || key === "hidden" || key === "value") node[key] = value;
    else node.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child !== null && child !== undefined && child !== false) node.append(child);
  }
  return node;
}

const fill = (node, ...children) => node.replaceChildren(...children.flat().filter(Boolean));
const $ = id => document.getElementById(id);
const short = key => { const s = key.toString(); return `${s.slice(0, 4)}…${s.slice(-4)}`; };
const sol = lamports => (lamports / LAMPORTS_PER_SOL).toFixed(4).replace(/0+$/, "").replace(/\.$/, "");
const explorer = (kind, value) => `https://explorer.solana.com/${kind}/${value}?cluster=devnet`;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const now = () => Math.floor(Date.now() / 1000);
const looksAlike = (a, b) => {
  const [x, y] = [a.toString(), b.toString()];
  return x !== y && x.slice(0, 4) === y.slice(0, 4) && x.slice(-4) === y.slice(-4);
};

function highlightDifferences(address, other) {
  const [text, against] = [address.toString(), other.toString()];
  return [...text].map((ch, i) => ch === against[i] ? ch : el("b", null, ch));
}

const localSigner = key => ({
  publicKey: key.publicKey,
  isWallet: false,
  signTransaction: async tx => { tx.partialSign(key); return tx; },
});

class Expired extends Error {}

async function submit(instructions, signer, { sponsored = false } = {}) {
  const attempts = signer.isWallet ? 1 : 4;
  let failure;
  for (let attempt = 0; attempt < attempts; attempt++) {
    let raw;
    let lastValidBlockHeight;
    try {
      const latest = await connection.getLatestBlockhash("confirmed");
      lastValidBlockHeight = latest.lastValidBlockHeight;
      const feePayer = sponsored ? funder.publicKey : signer.publicKey;
      let tx = new Transaction({ feePayer, blockhash: latest.blockhash, lastValidBlockHeight }).add(...instructions);
      tx = await signer.signTransaction(tx);
      if (sponsored) tx.partialSign(funder);
      raw = tx.serialize();
    } catch (error) {
      if (signer.isWallet) throw error;
      failure = error;
      await sleep(1500 * (attempt + 1));
      continue;
    }
    try {
      return await land(raw, lastValidBlockHeight);
    } catch (error) {
      failure = error;
      if (!(error instanceof Expired)) throw error;
    }
  }
  throw failure;
}

async function land(raw, lastValidBlockHeight) {
  let signature;
  for (let i = 0; i < 4 && !signature; i++) {
    try {
      signature = await connection.sendRawTransaction(raw, { skipPreflight: true, maxRetries: 3 });
    } catch (error) {
      if (i === 3) throw error;
      await sleep(1200 * (i + 1));
    }
  }
  for (let tick = 0; ; tick++) {
    await sleep(1000);
    let status = null;
    try {
      ({ value: [status] } = await connection.getSignatureStatuses([signature]));
    } catch {
      continue;
    }
    if (status && ["confirmed", "finalized"].includes(status.confirmationStatus)) {
      return { signature, ok: !status.err, logs: status.err ? await readLogs(signature) : [] };
    }
    if (tick % 4 === 3) {
      connection.sendRawTransaction(raw, { skipPreflight: true, maxRetries: 0 }).catch(() => {});
      const height = await connection.getBlockHeight("confirmed").catch(() => 0);
      if (height > lastValidBlockHeight) throw new Expired("Devnet dropped that transaction. Please try again.");
    }
  }
}

async function readLogs(signature) {
  for (let i = 0; i < 8; i++) {
    const tx = await connection.getTransaction(signature, { maxSupportedTransactionVersion: 0, commitment: "confirmed" }).catch(() => null);
    if (tx) return tx.meta.logMessages || [];
    await sleep(1000);
  }
  return [];
}

const reasonFrom = logs => logs.filter(l => l.includes("handshake:")).map(l => l.replace("Program log: handshake: ", "")).join(" ");

function concat(...parts) {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) { out.set(part, offset); offset += part.length; }
  return out;
}

function le64(value) {
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setBigInt64(0, BigInt(value), true);
  return bytes;
}

async function hold(signer, recipient, lamports, seconds) {
  const nonce = crypto.getRandomValues(new Uint8Array(8));
  const secret = crypto.getRandomValues(new Uint8Array(32));
  const [payment] = PublicKey.findProgramAddressSync([new TextEncoder().encode("payment"), signer.publicKey.toBytes(), nonce], programId);
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", secret));
  const result = await submit([new TransactionInstruction({
    programId,
    data: concat(Uint8Array.of(0), nonce, le64(lamports), le64(seconds), hash),
    keys: [
      { pubkey: signer.publicKey, isSigner: true, isWritable: true },
      { pubkey: payment, isSigner: false, isWritable: true },
      { pubkey: recipient, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
  })], signer);
  if (!result.ok) throw new Error(reasonFrom(result.logs) || "The hold transaction failed.");
  let state = null;
  for (let i = 0; i < 5 && !state; i++) {
    state = await readPayment(payment).catch(() => null);
    if (!state) await sleep(800);
  }
  return { payment, secret, recipient, signature: result.signature, deadline: state ? state.deadline : now() + seconds, lamports };
}

async function readPayment(payment) {
  const account = await connection.getAccountInfo(payment, "confirmed");
  if (!account || !account.owner.equals(programId)) return null;
  const view = new DataView(account.data.buffer, account.data.byteOffset);
  return {
    sender: new PublicKey(account.data.subarray(1, 33)),
    recipient: new PublicKey(account.data.subarray(33, 65)),
    lamports: Number(view.getBigUint64(97, true)),
    deadline: Number(view.getBigInt64(105, true)),
  };
}

const claimIx = (claimant, payment, sender, secret) => new TransactionInstruction({
  programId,
  data: concat(Uint8Array.of(1), secret),
  keys: [
    { pubkey: claimant, isSigner: true, isWritable: true },
    { pubkey: payment, isSigner: false, isWritable: true },
    { pubkey: sender, isSigner: false, isWritable: true },
  ],
});

const reclaimIx = (caller, payment, sender) => new TransactionInstruction({
  programId,
  data: Uint8Array.of(2),
  keys: [
    { pubkey: caller, isSigner: true, isWritable: true },
    { pubkey: payment, isSigner: false, isWritable: true },
    { pubkey: sender, isSigner: false, isWritable: true },
  ],
});

const toLinkSecret = bytes => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const fromLinkSecret = text => Uint8Array.from(atob(text.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0));
const claimLink = held => `${location.origin}${location.pathname}#claim=${held.payment.toString()}&s=${toLinkSecret(held.secret)}`;

function friendlyError(error) {
  const text = error && error.message ? error.message : String(error);
  if (/reject|denied|declined/i.test(text)) return "You declined that in your wallet, so nothing was sent.";
  if (/429|Too many/i.test(text)) return "Devnet is rate limiting right now. Wait a few seconds and try again.";
  if (/Failed to fetch|NetworkError/i.test(text)) return "Couldn't reach devnet. Check your connection and try again.";
  return text;
}

const TASKS = [
  ["lookalike", "Pay the address that sent you 0.000001 SOL. It's the lookalike."],
  ["scamClaim", "In the Scammer wallet, try to claim it."],
  ["refund", "Wait for the hold to end, or cancel it. Your money comes back."],
  ["friendHold", "Pay the address you sent 0.001 SOL to. That's your friend."],
  ["friendClaim", "In the Friend wallet, claim it from the link you sent."],
  ["direct", "Pay your friend again. It goes straight through."],
];

const demo = {
  started: false,
  starting: false,
  startError: "",
  balances: {},
  history: [],
  payments: [],
  txs: [],
  verified: false,
  done: {},
  busy: {},
  errors: {},
  sendError: "",
  sending: false,
};

const toInput = el("input", { class: "addr-input", placeholder: "Paste or use an address from Activity", autocomplete: "off", spellcheck: "false" });
const amountInput = el("input", { type: "number", min: String(MIN_AMOUNT), step: "0.001", value: String(DEMO_AMOUNT), inputmode: "decimal" });
const sendStatus = el("div");
toInput.oninput = updateSendStatus;

function parseKey(text) {
  try { return new PublicKey(text.trim()); } catch { return null; }
}

function updateSendStatus() {
  const key = parseKey(toInput.value);
  sendStatus.replaceChildren();
  if (key && looksAlike(key, friend.publicKey)) {
    sendStatus.append(el("div", { class: "alert" },
      el("strong", null, "This isn't the address you paid before."),
      el("div", { class: "compare-addr" }, el("span", null, "Paid"), friend.publicKey.toString()),
      el("div", { class: "compare-addr" }, el("span", null, "This"), highlightDifferences(key, friend.publicKey))));
  } else if (key && key.equals(friend.publicKey) && demo.verified) {
    sendStatus.append(el("div", { class: "good-box" }, el("strong", null, "Verified. "), "This address has claimed from you before, so there's no hold."));
  }
  renderSendButton();
}

const sendButtonSlot = el("div", { class: "actions" });

function renderSendButton() {
  const key = parseKey(toInput.value);
  const direct = key && key.equals(friend.publicKey) && demo.verified;
  const next = nextTask();
  const hinted = !demo.sending && ["lookalike", "friendHold", "direct"].includes(next);
  sendButtonSlot.replaceChildren(
    el("button", { class: `btn${hinted && key ? " next" : ""}`, disabled: demo.sending, onclick: send },
      demo.sending ? "Sending…" : direct ? "Send" : "Send with hold"),
    el("span", { class: "note", style: "margin:0" }, direct ? "No hold for verified addresses" : `Hold: ${DEMO_HOLD} seconds`));
}

function nextTask() {
  const task = TASKS.find(([id]) => !demo.done[id]);
  return task ? task[0] : null;
}

function logTx(who, what, signature, ok = true) {
  demo.txs.unshift({ who, what, signature, ok, at: new Date().toLocaleTimeString() });
}

async function refreshBalances() {
  try {
    const [a, b, c] = await Promise.all([you, friend, scammer].map(k => connection.getBalance(k.publicKey, "confirmed")));
    demo.balances = { you: a, friend: b, scammer: c };
  } catch {}
  render();
}

async function recoverOldHolds() {
  const open = read("sendguard-demo-open", []);
  for (const text of open) {
    const payment = new PublicKey(text);
    const state = await readPayment(payment).catch(() => null);
    if (!state) continue;
    const result = await submit([reclaimIx(you.publicKey, payment, you.publicKey)], localSigner(you)).catch(() => null);
    if (result && result.ok) logTx("You", "Recovered a hold left over from an earlier visit", result.signature);
  }
  write("sendguard-demo-open", []);
}

async function startDemo() {
  demo.starting = true;
  demo.startError = "";
  render();
  try {
    const balances = await Promise.all([you, friend, scammer].map(k => connection.getBalance(k.publicKey, "confirmed")));
    if (balances[1] > RECYCLE_ABOVE) {
      await submit([SystemProgram.transfer({ fromPubkey: friend.publicKey, toPubkey: funder.publicKey, lamports: balances[1] - RECYCLE_KEEP })], localSigner(friend));
    }
    const topUps = [];
    if (balances[0] < YOU_MINIMUM) topUps.push([you, YOU_TOP_UP]);
    if (balances[1] < FEE_FLOOR) topUps.push([friend, GAS_TOP_UP]);
    if (balances[2] < FEE_FLOOR) topUps.push([scammer, GAS_TOP_UP]);
    if (topUps.length) {
      const funded = await submit(topUps.map(([key, lamports]) => SystemProgram.transfer({ fromPubkey: funder.publicKey, toPubkey: key.publicKey, lamports })), localSigner(funder));
      if (!funded.ok) throw new Error("The shared demo wallet is out of test SOL.");
      logTx("Demo", "Funded the demo wallets", funded.signature);
    }
    await recoverOldHolds();
    const paid = await submit([SystemProgram.transfer({ fromPubkey: you.publicKey, toPubkey: friend.publicKey, lamports: HISTORY_AMOUNT })], localSigner(you));
    if (!paid.ok) throw new Error("Couldn't set up your wallet history.");
    logTx("You", "Paid your friend 0.001 SOL earlier", paid.signature);
    const dust = await submit([SystemProgram.transfer({ fromPubkey: scammer.publicKey, toPubkey: you.publicKey, lamports: 1000 })], localSigner(scammer));
    if (!dust.ok) throw new Error("Couldn't set up your wallet history.");
    logTx("Scammer", "Sent you 0.000001 SOL from the lookalike", dust.signature);
    demo.history = [
      { label: "Received 0.000001 SOL", from: "from", key: scammer.publicKey, signature: dust.signature, when: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) },
      { label: "Sent 0.001 SOL", from: "to", key: friend.publicKey, signature: paid.signature, when: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) },
    ];
    demo.started = true;
  } catch (error) {
    demo.startError = friendlyError(error);
  }
  demo.starting = false;
  $("startBox").replaceChildren();
  await refreshBalances();
}

async function send() {
  demo.sendError = "";
  const key = parseKey(toInput.value);
  const amount = Number(amountInput.value);
  if (!key) { demo.sendError = "Enter a valid Solana address."; render(); return; }
  if (!(amount >= MIN_AMOUNT)) { demo.sendError = `Send at least ${MIN_AMOUNT} SOL.`; render(); return; }
  const lamports = Math.round(amount * LAMPORTS_PER_SOL);
  if (demo.balances.you !== undefined && lamports > demo.balances.you - 0.003 * LAMPORTS_PER_SOL) {
    demo.sendError = "That's more than this wallet can send. Try a smaller amount.";
    render();
    return;
  }
  demo.sending = true;
  render();
  try {
    if (key.equals(friend.publicKey) && demo.verified) {
      const result = await submit([SystemProgram.transfer({ fromPubkey: you.publicKey, toPubkey: key, lamports })], localSigner(you));
      if (!result.ok) throw new Error("The payment failed.");
      logTx("You", `Sent ${sol(lamports)} SOL straight to your friend`, result.signature);
      demo.done.direct = true;
    } else {
      const held = await hold(localSigner(you), key, lamports, DEMO_HOLD);
      write("sendguard-demo-open", [...read("sendguard-demo-open", []), held.payment.toString()]);
      demo.payments.unshift({ ...held, status: "held", messages: {} });
      const label = key.equals(scammer.publicKey) ? "the lookalike" : key.equals(friend.publicKey) ? "your friend" : short(key);
      logTx("You", `Held ${sol(lamports)} SOL for ${label}`, held.signature);
      if (key.equals(scammer.publicKey)) demo.done.lookalike = true;
      if (key.equals(friend.publicKey)) demo.done.friendHold = true;
    }
    toInput.value = "";
    updateSendStatus();
  } catch (error) {
    demo.sendError = friendlyError(error);
  }
  demo.sending = false;
  await refreshBalances();
}

function forgetOpen(payment) {
  write("sendguard-demo-open", read("sendguard-demo-open", []).filter(text => text !== payment.payment.toString()));
}

async function act(id, run) {
  if (demo.busy[id]) return;
  demo.busy[id] = true;
  demo.errors[id] = "";
  render();
  try {
    await run();
  } catch (error) {
    demo.errors[id] = friendlyError(error);
  }
  demo.busy[id] = false;
  await refreshBalances();
}

function scammerClaim(p) {
  return act(`scam-${p.signature}`, async () => {
    const guess = crypto.getRandomValues(new Uint8Array(32));
    const result = await submit([claimIx(scammer.publicKey, p.payment, you.publicKey, guess)], localSigner(scammer));
    if (result.ok) throw new Error("Unexpected: the claim succeeded.");
    const reason = reasonFrom(result.logs) || "rejected";
    p.messages.scammer = reason;
    logTx("Scammer", `Tried to claim: rejected, ${reason}`, result.signature, false);
    if (p.recipient.equals(scammer.publicKey)) demo.done.scamClaim = true;
  });
}

function friendClaim(p) {
  return act(`friend-${p.signature}`, async () => {
    const result = await submit([claimIx(friend.publicKey, p.payment, you.publicKey, p.secret)], localSigner(friend));
    if (!result.ok) {
      const reason = reasonFrom(result.logs) || "rejected";
      p.messages.friend = reason;
      logTx("Friend", `Tried to claim: rejected, ${reason}`, result.signature, false);
      return;
    }
    p.status = "claimed";
    forgetOpen(p);
    demo.verified = true;
    demo.done.friendClaim = true;
    logTx("Friend", `Claimed ${sol(p.lamports)} SOL`, result.signature);
  });
}

function reclaim(p, signer, who, label) {
  return act(`reclaim-${p.signature}`, async () => {
    const state = await readPayment(p.payment);
    if (!state) {
      p.status = "refunded";
      forgetOpen(p);
      return;
    }
    const result = await submit([reclaimIx(signer.publicKey, p.payment, you.publicKey)], signer);
    if (!result.ok) throw new Error(reasonFrom(result.logs) || "The refund failed.");
    p.status = "refunded";
    forgetOpen(p);
    logTx(who, label(p), result.signature);
    if (p.recipient.equals(scammer.publicKey)) demo.done.refund = true;
  });
}

function tick() {
  for (const p of demo.payments) {
    if (p.status !== "held") continue;
    const left = p.deadline - now();
    for (const node of document.querySelectorAll(`[data-timer="${p.signature}"]`)) {
      node.textContent = left > 0 ? `${left}s left` : "Returning to you…";
    }
    for (const node of document.querySelectorAll(`[data-bar="${p.signature}"]`)) {
      node.style.width = `${Math.max(0, Math.min(100, (left / DEMO_HOLD) * 100))}%`;
    }
    if (left <= -3 && !demo.busy[`reclaim-${p.signature}`] && (!p.retryAt || p.retryAt <= now())) {
      p.retryAt = now() + 5;
      reclaim(p, localSigner(funder), "Anyone", q => `Hold ended: ${sol(q.lamports)} SOL returned to you`);
    }
  }
}

function walletHead(name, key) {
  return el("div", { class: "w-head" }, el("b", null, name),
    el("a", { href: explorer("address", key.publicKey.toString()), target: "_blank", rel: "noopener", title: key.publicKey.toString() }, short(key.publicKey)));
}

function balanceRow(lamports) {
  return el("div", { class: "w-bal" }, lamports === undefined ? "…" : sol(lamports), " ", el("small", null, "SOL"));
}

function renderYou() {
  const held = demo.payments.filter(p => p.status === "held");
  const done = demo.payments.filter(p => p.status !== "held");
  return el("div", { class: "wallet" },
    walletHead("You", you),
    balanceRow(demo.balances.you),
    el("div", { class: "w-sec" },
      el("div", { class: "w-title" }, "Send"),
      el("label", { class: "field", style: "margin-top:0" }, el("span", null, "To"), toInput),
      sendStatus,
      el("label", { class: "field" }, el("span", null, "Amount (SOL)"), amountInput),
      sendButtonSlot,
      demo.sendError && el("div", { class: "error", role: "alert" }, demo.sendError)),
    held.length > 0 && el("div", { class: "w-sec" },
      el("div", { class: "w-title" }, "On hold"),
      held.map(p => el("div", { class: "card hold" },
        el("strong", null, `${sol(p.lamports)} SOL for ${short(p.recipient)}`),
        el("span", { class: "timer", "data-timer": p.signature }),
        el("div", { class: "bar" }, el("i", { "data-bar": p.signature })),
        el("div", null, "Secret link sent to your friend."),
        el("div", { class: "actions" },
          el("button", { class: "btn quiet small", disabled: demo.busy[`reclaim-${p.signature}`], onclick: () => reclaim(p, localSigner(you), "You", q => `Cancelled: ${sol(q.lamports)} SOL returned to you`) }, demo.busy[`reclaim-${p.signature}`] ? "Returning…" : "Cancel")),
        demo.errors[`reclaim-${p.signature}`] && el("div", { class: "error" }, demo.errors[`reclaim-${p.signature}`])))),
    el("div", { class: "w-sec" },
      el("div", { class: "w-title" }, "Activity"),
      done.map(p => el("div", { class: `card ${p.status === "claimed" ? "good" : ""}` },
        p.status === "claimed" ? `Claimed: ${sol(p.lamports)} SOL went to ${short(p.recipient)}` : `Returned: ${sol(p.lamports)} SOL didn't go to ${short(p.recipient)}`)),
      demo.history.map(row => el("div", { class: "item" },
        el("div", null, row.label, " ", row.from, " ", el("span", { class: "mono" }, short(row.key)), el("small", null, row.when, " · ", el("a", { href: explorer("tx", row.signature), target: "_blank", rel: "noopener" }, "View ↗"))),
        el("button", { class: "btn quiet small", onclick: () => { toInput.value = row.key.toString(); updateSendStatus(); } }, "Use")))));
}

function renderFriend() {
  const open = demo.payments.filter(p => p.status === "held");
  const claimed = demo.payments.filter(p => p.status === "claimed");
  const hinted = nextTask() === "friendClaim";
  return el("div", { class: "wallet" },
    walletHead("Friend", friend),
    balanceRow(demo.balances.friend),
    el("div", { class: "w-sec" },
      el("div", { class: "w-title" }, "Messages from you"),
      open.length === 0 && claimed.length === 0 && el("div", { class: "empty" }, "No messages yet."),
      open.map(p => {
        const id = `friend-${p.signature}`;
        return el("div", { class: "msg" },
          `Sent you ${sol(p.lamports)} SOL. Claim it here:`,
          el("code", null, claimLink(p)),
          el("div", { class: "actions" },
            el("button", { class: `btn small${hinted && p.recipient.equals(friend.publicKey) ? " next" : ""}`, disabled: demo.busy[id], onclick: () => friendClaim(p) }, demo.busy[id] ? "Claiming…" : "Claim")),
          p.messages.friend && el("div", { class: "card bad" }, el("strong", null, "Rejected on-chain"), "This payment went to a different address, so this wallet can't claim it."),
          demo.errors[id] && el("div", { class: "error" }, demo.errors[id]));
      }),
      claimed.map(p => el("div", { class: "card good" }, `Claimed ${sol(p.lamports)} SOL`))));
}

function renderScammer() {
  const incoming = demo.payments.filter(p => p.recipient.equals(scammer.publicKey));
  const hinted = nextTask() === "scamClaim";
  return el("div", { class: "wallet" },
    walletHead("Scammer", scammer),
    balanceRow(demo.balances.scammer),
    el("div", { class: "w-sec" },
      el("div", { class: "w-title" }, "Payments to this address"),
      incoming.length === 0 && el("div", { class: "empty" }, "Nothing yet. In a normal wallet, a mistaken payment would arrive here instantly."),
      incoming.map(p => {
        const id = `scam-${p.signature}`;
        return el("div", { class: `card ${p.status === "held" ? "hold" : ""}` },
          el("strong", null, `${sol(p.lamports)} SOL`),
          p.status === "held" ? el("span", { class: "timer", "data-timer": p.signature }) : "Went back to the sender. Nothing received.",
          p.status === "held" && el("div", null, "On hold. This wallet doesn't have the secret."),
          p.status === "held" && el("div", { class: "actions" },
            el("button", { class: `btn small${hinted ? " next" : ""}`, disabled: demo.busy[id], onclick: () => scammerClaim(p) }, demo.busy[id] ? "Claiming…" : "Try to claim")),
          p.messages.scammer && el("div", { class: "card bad" }, el("strong", null, "Rejected on-chain"), `Program said: ${p.messages.scammer}`),
          demo.errors[id] && el("div", { class: "error" }, demo.errors[id]));
      })));
}

function renderGuide() {
  const next = TASKS.find(([id]) => !demo.done[id]);
  const count = TASKS.filter(([id]) => demo.done[id]).length;
  const text = !demo.started ? "Set up the demo to get started." : next ? next[1] : "That's everything. You can keep sending, or reload to start over.";
  $("guide").replaceChildren(el("b", null, demo.started && next ? `Next: ${text}` : text), el("span", null, `${count} of ${TASKS.length} done`));
}

function renderTxs() {
  const box = $("txs");
  box.hidden = demo.txs.length === 0;
  if (box.hidden) return;
  box.replaceChildren(el("table", null,
    el("thead", null, el("tr", null, ["Time", "Wallet", "What happened", "Result", "Transaction"].map(h => el("th", null, h)))),
    el("tbody", null, demo.txs.map(tx => el("tr", null,
      el("td", { style: "white-space:nowrap" }, tx.at),
      el("td", null, tx.who),
      el("td", null, tx.what),
      el("td", { class: tx.ok ? "ok" : "fail" }, tx.ok ? "Success" : "Failed"),
      el("td", { class: "sig" }, el("a", { href: explorer("tx", tx.signature), target: "_blank", rel: "noopener" }, `${tx.signature.slice(0, 10)}… ↗`)))))));
}

function renderStart() {
  const box = $("startBox");
  $("wallets").hidden = !demo.started;
  if (demo.started) { box.replaceChildren(); return; }
  fill(box,
    el("div", { class: "form", style: "max-width:none" },
      el("p", { style: "margin-bottom:8px" }, "This creates a wallet for you with test SOL and adds both addresses to its history."),
      el("button", { class: "btn next", disabled: demo.starting, onclick: startDemo }, demo.starting ? "Setting up, about 15 seconds…" : "Set up the demo"),
      demo.startError && el("div", { class: "error", role: "alert" }, demo.startError)));
}

function render() {
  renderGuide();
  renderStart();
  renderTxs();
  if (demo.started) {
    renderSendButton();
    fill($("wallets"), renderYou(), renderFriend(), renderScammer());
    tick();
  }
}

function findWallet() {
  return [
    window.phantom && window.phantom.solana,
    window.solflare,
    window.backpack,
    window.solana,
  ].find(provider => provider && provider.signTransaction);
}

async function connectWallet() {
  const provider = findWallet();
  if (!provider) throw new Error("No Solana wallet found in this browser. Install Phantom or Solflare and switch it to devnet.");
  const response = await provider.connect();
  const publicKey = new PublicKey((response && response.publicKey) || provider.publicKey);
  return {
    publicKey,
    isWallet: true,
    signTransaction: tx => provider.signTransaction(tx),
  };
}

const HOLDS = [["10 minutes", 600], ["1 hour", 3600], ["24 hours", 86400]];
const app = { wallet: null, balance: 0, result: null };

const verifiedContacts = () => read("handshake-verified", []);
const sentPayments = () => read("handshake-sent", []);

async function syncSent() {
  const sent = sentPayments();
  const open = [];
  const verified = new Set(verifiedContacts());
  for (const item of sent) {
    const state = await readPayment(new PublicKey(item.payment));
    if (state) {
      open.push({ ...item, deadline: state.deadline });
      continue;
    }
    const [latest] = await connection.getSignaturesForAddress(new PublicKey(item.payment), { limit: 1 });
    if (!latest) continue;
    const tx = await connection.getTransaction(latest.signature, { maxSupportedTransactionVersion: 0, commitment: "confirmed" });
    if (tx && (tx.meta.logMessages || []).some(line => line.includes("handshake: claimed"))) verified.add(item.recipient);
  }
  write("handshake-sent", open);
  write("handshake-verified", [...verified]);
}

async function renderRealApp() {
  const root = $("realApp");
  if (!app.wallet) {
    const error = el("div", { class: "error", role: "alert" });
    root.replaceChildren(
      el("p", null, "Switch your wallet to devnet, then connect."),
      el("div", { class: "actions" }, el("button", { class: "btn", onclick: async () => {
        try {
          app.wallet = await connectWallet();
          await renderRealApp();
        } catch (e) {
          error.textContent = friendlyError(e);
        }
      } }, "Connect wallet")),
      error,
      el("p", { class: "note" }, "Phantom, Solflare or Backpack. ", el("a", { href: "https://faucet.solana.com", target: "_blank", rel: "noopener" }, "Get test SOL ↗")));
    return;
  }

  await syncSent().catch(() => {});
  app.balance = await connection.getBalance(app.wallet.publicKey, "confirmed").catch(() => 0);
  const verified = new Set(verifiedContacts());
  const known = [...verified, ...sentPayments().map(p => p.recipient)];

  const recipient = el("input", { class: "addr-input", placeholder: "Recipient's Solana address", autocomplete: "off", spellcheck: "false" });
  const amount = el("input", { type: "number", min: String(MIN_AMOUNT), step: "0.001", value: "0.01", inputmode: "decimal" });
  const duration = el("select", null, HOLDS.map(([label, seconds]) => el("option", { value: String(seconds) }, label)));
  const status = el("div");
  const error = el("div", { class: "error", role: "alert" });
  const sendButton = el("button", { class: "btn" }, "Hold and create link");

  const parseRecipient = () => parseKey(recipient.value);

  recipient.oninput = () => {
    const key = parseRecipient();
    status.replaceChildren();
    sendButton.textContent = "Hold and create link";
    if (!key) return;
    if (verified.has(key.toString())) {
      sendButton.textContent = "Send";
      status.append(el("div", { class: "good-box" }, el("strong", null, "Verified. "), "This address has already claimed from you."));
      return;
    }
    const twin = known.find(address => looksAlike(key, address));
    if (twin) {
      status.append(el("div", { class: "alert" },
        el("strong", null, "This isn't the address you used before."),
        el("div", { class: "compare-addr" }, el("span", null, "Used"), twin),
        el("div", { class: "compare-addr" }, el("span", null, "This"), highlightDifferences(key, twin))));
    }
  };

  sendButton.onclick = async () => {
    error.textContent = "";
    const key = parseRecipient();
    const value = Number(amount.value);
    if (!key) { error.textContent = "Enter a valid Solana address."; return; }
    if (!(value >= MIN_AMOUNT)) { error.textContent = `Send at least ${MIN_AMOUNT} SOL, so a new wallet can receive it.`; return; }
    const lamports = Math.round(value * LAMPORTS_PER_SOL);
    sendButton.disabled = true;
    const label = sendButton.textContent;
    sendButton.textContent = "Confirm in your wallet…";
    try {
      if (verified.has(key.toString())) {
        const result = await submit([SystemProgram.transfer({ fromPubkey: app.wallet.publicKey, toPubkey: key, lamports })], app.wallet);
        if (!result.ok) throw new Error("The payment failed. Does your wallet have devnet SOL?");
        app.result = { kind: "direct", signature: result.signature, lamports };
      } else {
        const held = await hold(app.wallet, key, lamports, Number(duration.value));
        const link = claimLink(held);
        write("handshake-sent", [...sentPayments(), { payment: held.payment.toString(), sender: app.wallet.publicKey.toString(), recipient: key.toString(), lamports, link }]);
        app.result = { kind: "held", signature: held.signature, link, lamports };
      }
      await renderRealApp();
    } catch (e) {
      error.textContent = friendlyError(e);
      sendButton.disabled = false;
      sendButton.textContent = label;
    }
  };

  const sent = sentPayments().filter(item => item.sender === app.wallet.publicKey.toString());
  fill(root,
    el("div", { class: "pending-item", style: "border-top:0;padding-top:0" },
      el("span", null, "Connected ", el("span", { class: "mono" }, short(app.wallet.publicKey))),
      el("span", null, `${sol(app.balance)} SOL on devnet`)),
    app.balance < 10000 && el("p", { class: "note" }, "This wallet has no devnet SOL. ", el("a", { href: "https://faucet.solana.com", target: "_blank", rel: "noopener" }, "Get test SOL ↗")),
    resultBox(),
    el("label", { class: "field" }, el("span", null, "To"), recipient),
    status,
    el("div", { class: "row2" },
      el("label", { class: "field" }, el("span", null, "Amount (test SOL)"), amount),
      el("label", { class: "field" }, el("span", null, "Return if unclaimed after"), duration)),
    el("div", { class: "actions" }, sendButton),
    error,
    sent.length > 0 && el("div", { style: "margin-top:14px" },
      el("div", { class: "label" }, "Waiting to be claimed"),
      sent.map(pendingRow)));
}

function pendingRow(item) {
  const error = el("div", { class: "error", role: "alert" });
  const cancel = el("button", { class: "btn quiet small", onclick: async () => {
    cancel.disabled = true;
    try {
      const payment = new PublicKey(item.payment);
      const result = await submit([reclaimIx(app.wallet.publicKey, payment, app.wallet.publicKey)], app.wallet);
      if (!result.ok) throw new Error(reasonFrom(result.logs) || "The cancel failed.");
      app.result = { kind: "cancelled", signature: result.signature, lamports: item.lamports };
      write("handshake-sent", sentPayments().filter(p => p.payment !== item.payment));
      await renderRealApp();
    } catch (e) {
      error.textContent = friendlyError(e);
      cancel.disabled = false;
    }
  } }, "Get it back");
  const deadline = item.deadline ? new Date(item.deadline * 1000).toLocaleString() : "";
  return el("div", { class: "pending-item" },
    el("span", null, `${sol(item.lamports)} SOL to `, el("span", { class: "mono" }, short(item.recipient)), deadline && el("div", { class: "note", style: "margin:0" }, `Hold ends ${deadline}`)),
    cancel, error);
}

function resultBox() {
  const result = app.result;
  if (!result) return null;
  const proof = el("a", { href: explorer("tx", result.signature), target: "_blank", rel: "noopener" }, "View transaction ↗");
  if (result.kind === "direct") return el("div", { class: "good-box" }, el("strong", null, `Sent ${sol(result.lamports)} SOL. `), proof);
  if (result.kind === "cancelled") return el("div", { class: "good-box" }, el("strong", null, `${sol(result.lamports)} SOL returned to your wallet. `), proof);
  const copy = el("button", { class: "btn quiet small", onclick: async () => {
    try {
      await navigator.clipboard.writeText(result.link);
      copy.textContent = "Copied";
    } catch {
      copy.textContent = "Select the link to copy it";
    }
  } }, "Copy link");
  return el("div", { class: "good-box" },
    el("strong", null, `${sol(result.lamports)} SOL is on hold. Send this link to the person you're paying:`),
    el("div", { class: "link-box" }, result.link),
    el("div", { class: "actions" },
      copy,
      el("a", { class: "btn quiet small", href: `https://wa.me/?text=${encodeURIComponent(`I sent you test SOL with SendGuard. Claim it here: ${result.link}`)}`, target: "_blank", rel: "noopener" }, "Share on WhatsApp"),
      proof));
}

async function renderClaimPage(paymentText, secretText) {
  const paymentKey = new PublicKey(paymentText);
  const secret = fromLinkSecret(secretText);
  const body = el("div", { class: "scene" });
  $("page").replaceChildren(body);
  body.append(el("p", { class: "say" }, "Checking this payment on devnet…"));

  const state = await readPayment(paymentKey).catch(() => null);
  if (!state) {
    body.replaceChildren(el("h3", null, "This payment is no longer waiting"), el("p", { class: "say" }, "It has already been claimed or returned to the sender."));
    return;
  }

  const expired = state.deadline < now();
  const error = el("div", { class: "error", role: "alert" });
  const details = [
    el("div", { class: "label" }, "Someone sent you a payment"),
    el("h3", null, `${sol(state.lamports)} test SOL`),
    el("p", { class: "say" }, expired
      ? "The hold has ended, so this can only go back to the sender now."
      : `Claim it with the wallet it was sent to, before ${new Date(state.deadline * 1000).toLocaleString()}.`),
    el("div", { class: "compare-addr" }, el("span", null, "From"), state.sender.toString()),
    el("div", { class: "compare-addr" }, el("span", null, "For"), state.recipient.toString()),
  ];

  const claimWith = async (signer, button, options) => {
    button.disabled = true;
    const label = button.textContent;
    button.textContent = "Claiming…";
    error.textContent = "";
    try {
      const result = await submit([claimIx(signer.publicKey, paymentKey, state.sender, secret)], signer, options);
      if (!result.ok) throw new Error(reasonFrom(result.logs) || "The claim was rejected.");
      body.replaceChildren(
        el("h3", null, "Claimed"),
        el("p", { class: "say" }, `${sol(state.lamports)} test SOL is in your wallet.`),
        el("div", { class: "actions" }, el("a", { class: "btn", href: explorer("tx", result.signature), target: "_blank", rel: "noopener" }, "View transaction ↗")));
    } catch (e) {
      error.textContent = friendlyError(e);
      button.disabled = false;
      button.textContent = label;
    }
  };

  const walletButton = el("button", { class: "btn", onclick: async () => {
    try {
      const wallet = await connectWallet();
      if (!wallet.publicKey.equals(state.recipient)) {
        error.textContent = `This payment is for ${short(state.recipient)}, but your wallet is ${short(wallet.publicKey)}. Switch to the right wallet.`;
        return;
      }
      await claimWith(wallet, walletButton, { sponsored: true });
    } catch (e) {
      error.textContent = friendlyError(e);
    }
  } }, "Connect wallet and claim");
  const isDemoFriend = state.recipient.equals(friend.publicKey);
  const demoButton = isDemoFriend ? el("button", { class: "btn quiet", onclick: () => claimWith(localSigner(friend), demoButton) }, "Claim as the demo friend") : null;

  fill(body, ...details, !expired && el("div", { class: "actions" }, walletButton, demoButton), !expired && el("p", { class: "note" }, "Your wallet signs the claim. The demo pays the devnet fee, so a new wallet can claim too."), error);
}

function start() {
  const params = new URLSearchParams(location.hash.slice(1));
  if (params.get("claim") && params.get("s")) {
    renderClaimPage(params.get("claim"), params.get("s")).catch(e => {
      document.querySelector(".scene").replaceChildren(el("h3", null, "This link isn't valid"), el("p", { class: "say" }, friendlyError(e)));
    });
    return;
  }
  $("programLink").href = explorer("address", cfg.programId);
  fill($("twins"),
    el("small", null, "These are two different addresses. Wallets usually show only the ends, 8E1W…mtna, so they look identical."),
    el("div", null, friend.publicKey.toString()),
    el("div", null, highlightDifferences(scammer.publicKey, friend.publicKey)));
  render();
  setInterval(tick, 1000);
  renderRealApp();
}

start();
