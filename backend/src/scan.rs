use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

const IGNORED: &[&str] = &[
    "11111111111111111111111111111111",
    "ComputeBudget111111111111111111111111111111",
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
    "So11111111111111111111111111111111111111112",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Tx {
    pub signature: String,
    pub time: Option<i64>,
    pub signers: Vec<String>,
    pub gained: Vec<String>,
    pub lost: Vec<String>,
    pub wallet_lamports: i128,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Finding {
    pub lookalike: String,
    pub imitates: String,
    pub matching_start: usize,
    pub matching_end: usize,
    pub severity: &'static str,
    pub you_paid_it: bool,
    pub times_seen: usize,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    pub example_signature: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub address: String,
    pub scanned: usize,
    pub failed_skipped: usize,
    pub oldest: Option<i64>,
    pub newest: Option<i64>,
    pub addresses_you_paid: usize,
    pub findings: Vec<Finding>,
}

pub fn parse(wallet: &str, signature: &str, tx: &Value) -> Option<Tx> {
    let meta = &tx["meta"];
    if !meta["err"].is_null() {
        return None;
    }
    let keys = tx["transaction"]["message"]["accountKeys"].as_array()?;
    let mut signers = Vec::new();
    let mut delta: BTreeMap<String, i128> = BTreeMap::new();
    let mut wallet_lamports = 0;
    let (pre, post) = (
        meta["preBalances"].as_array()?,
        meta["postBalances"].as_array()?,
    );
    for (i, key) in keys.iter().enumerate() {
        let address = key["pubkey"].as_str()?.to_string();
        if key["signer"].as_bool() == Some(true) {
            signers.push(address.clone());
        }
        let change = post.get(i).and_then(Value::as_i64).unwrap_or(0) as i128
            - pre.get(i).and_then(Value::as_i64).unwrap_or(0) as i128;
        if address == wallet {
            wallet_lamports = change;
        }
        if change != 0 {
            *delta.entry(address).or_default() += change.signum();
        }
    }
    let mut tokens: BTreeMap<(String, String), i128> = BTreeMap::new();
    for (list, sign) in [("preTokenBalances", -1i128), ("postTokenBalances", 1)] {
        for balance in meta[list].as_array().into_iter().flatten() {
            let (Some(owner), Some(mint)) = (balance["owner"].as_str(), balance["mint"].as_str())
            else {
                continue;
            };
            let amount: i128 = balance["uiTokenAmount"]["amount"]
                .as_str()
                .and_then(|a| a.parse().ok())
                .unwrap_or(0);
            *tokens
                .entry((owner.to_string(), mint.to_string()))
                .or_default() += sign * amount;
        }
    }
    let mut gained: HashSet<String> = HashSet::new();
    let mut lost: HashSet<String> = HashSet::new();
    for (address, sign) in delta {
        if sign > 0 {
            gained.insert(address);
        } else {
            lost.insert(address);
        }
    }
    for ((owner, _), change) in tokens {
        if change > 0 {
            gained.insert(owner);
        } else if change < 0 {
            lost.insert(owner);
        }
    }
    let mut gained: Vec<_> = gained.into_iter().collect();
    let mut lost: Vec<_> = lost.into_iter().collect();
    gained.sort();
    lost.sort();
    Some(Tx {
        signature: signature.to_string(),
        time: tx["blockTime"].as_i64(),
        signers,
        gained,
        lost,
        wallet_lamports,
    })
}

fn shared(a: &str, b: &str) -> (usize, usize) {
    let start = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let end = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    (start, end)
}

fn severity(start: usize, end: usize) -> Option<&'static str> {
    if start >= 4 && end >= 4 {
        Some("high")
    } else if start >= 2 && end >= 2 && start + end >= 5 {
        Some("medium")
    } else if start >= 5 || end >= 5 {
        Some("low")
    } else {
        None
    }
}

#[derive(Default)]
struct Seen {
    count: usize,
    first: Option<i64>,
    last: Option<i64>,
    example: String,
}

impl Seen {
    fn add(&mut self, tx: &Tx) {
        self.count += 1;
        if self.example.is_empty() {
            self.example = tx.signature.clone();
        }
        if let Some(t) = tx.time {
            self.first = Some(self.first.map_or(t, |f| f.min(t)));
            self.last = Some(self.last.map_or(t, |l| l.max(t)));
        }
    }
}

pub fn scan(wallet: &str, txs: &[Tx], failed_skipped: usize) -> Report {
    let ignored = |a: &str| a == wallet || IGNORED.contains(&a) || a.starts_with("Sysvar");
    let mut paid: HashMap<&str, Seen> = HashMap::new();
    let mut strangers: HashMap<&str, Seen> = HashMap::new();
    for tx in txs {
        if tx.signers.iter().any(|s| s == wallet) {
            for address in tx.gained.iter().filter(|a| !ignored(a)) {
                paid.entry(address).or_default().add(tx);
            }
        } else {
            let involved: HashSet<&str> = tx
                .signers
                .iter()
                .chain(&tx.lost)
                .chain(&tx.gained)
                .map(String::as_str)
                .filter(|a| !ignored(a))
                .collect();
            for address in involved {
                strangers.entry(address).or_default().add(tx);
            }
        }
    }

    let mut findings = Vec::new();
    for (stranger, seen) in &strangers {
        if paid.contains_key(stranger) {
            continue;
        }
        let best = paid
            .keys()
            .filter_map(|known| {
                let (start, end) = shared(stranger, known);
                severity(start, end).map(|level| (start + end, start, end, level, *known))
            })
            .max();
        if let Some((_, start, end, level, known)) = best {
            findings.push(Finding {
                lookalike: stranger.to_string(),
                imitates: known.to_string(),
                matching_start: start,
                matching_end: end,
                severity: level,
                you_paid_it: false,
                times_seen: seen.count,
                first_seen: seen.first,
                last_seen: seen.last,
                example_signature: seen.example.clone(),
            });
        }
    }
    let mut known: Vec<_> = paid.iter().collect();
    known.sort_by_key(|(address, seen)| (seen.first, **address));
    for (i, (later, seen)) in known.iter().enumerate() {
        for (earlier, _) in &known[..i] {
            let (start, end) = shared(later, earlier);
            if let Some(level) = severity(start, end) {
                findings.push(Finding {
                    lookalike: later.to_string(),
                    imitates: earlier.to_string(),
                    matching_start: start,
                    matching_end: end,
                    severity: level,
                    you_paid_it: true,
                    times_seen: seen.count,
                    first_seen: seen.first,
                    last_seen: seen.last,
                    example_signature: seen.example.clone(),
                });
            }
        }
    }
    findings.sort_by(|a, b| {
        (
            b.you_paid_it,
            b.matching_start + b.matching_end,
            &a.lookalike,
        )
            .cmp(&(
                a.you_paid_it,
                a.matching_start + a.matching_end,
                &b.lookalike,
            ))
    });

    let times: Vec<i64> = txs.iter().filter_map(|t| t.time).collect();
    Report {
        address: wallet.to_string(),
        scanned: txs.len(),
        failed_skipped,
        oldest: times.iter().min().copied(),
        newest: times.iter().max().copied(),
        addresses_you_paid: paid.len(),
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WALLET: &str = "6VSswVv8ksjSabm5EEpMM6uH4GdiwAtUyTxX5QstYqcC";
    const FRIEND: &str = "8E1WmbHQgUPyE8W26tQkNHCrvWviViZBFTxAc2AVmtna";
    const TWIN: &str = "8E1WUCxqmPe2ZGKNd9JGpRPrSUHtHRqCWZSd9NSbmtna";
    const OTHER: &str = "56JvGkJWYEu291a1L3okqob27i3qb7hSXMx5TurHcFpM";

    fn tx(signature: &str, time: i64, signer: &str, gained: &[&str], lost: &[&str]) -> Tx {
        Tx {
            signature: signature.into(),
            time: Some(time),
            signers: vec![signer.into()],
            gained: gained.iter().map(|s| s.to_string()).collect(),
            lost: lost.iter().map(|s| s.to_string()).collect(),
            wallet_lamports: 0,
        }
    }

    #[test]
    fn dust_from_a_lookalike_of_someone_you_paid_is_flagged() {
        let txs = [
            tx("pay", 100, WALLET, &[FRIEND], &[WALLET]),
            tx("dust", 200, TWIN, &[WALLET], &[TWIN]),
        ];
        let report = scan(WALLET, &txs, 0);
        assert_eq!(report.findings.len(), 1);
        let finding = &report.findings[0];
        assert_eq!(
            (finding.lookalike.as_str(), finding.imitates.as_str()),
            (TWIN, FRIEND)
        );
        assert_eq!((finding.matching_start, finding.matching_end), (4, 4));
        assert_eq!(finding.severity, "high");
        assert!(!finding.you_paid_it);
        assert_eq!(finding.example_signature, "dust");
    }

    #[test]
    fn unrelated_senders_are_not_flagged() {
        let txs = [
            tx("pay", 100, WALLET, &[FRIEND], &[WALLET]),
            tx("in", 200, OTHER, &[WALLET], &[OTHER]),
        ];
        assert!(scan(WALLET, &txs, 0).findings.is_empty());
    }

    #[test]
    fn a_lookalike_with_nobody_to_imitate_is_not_flagged() {
        let txs = [tx("dust", 200, TWIN, &[WALLET], &[TWIN])];
        assert!(scan(WALLET, &txs, 0).findings.is_empty());
    }

    #[test]
    fn paying_the_lookalike_is_reported_as_paid() {
        let txs = [
            tx("pay", 100, WALLET, &[FRIEND], &[WALLET]),
            tx("dust", 200, TWIN, &[WALLET], &[TWIN]),
            tx("mistake", 300, WALLET, &[TWIN], &[WALLET]),
        ];
        let report = scan(WALLET, &txs, 0);
        assert_eq!(report.findings.len(), 1);
        assert!(report.findings[0].you_paid_it);
        assert_eq!(report.findings[0].lookalike, TWIN);
        assert_eq!(report.findings[0].imitates, FRIEND);
        assert_eq!(report.findings[0].example_signature, "mistake");
    }

    #[test]
    fn repeated_dust_is_counted() {
        let txs = [
            tx("pay", 100, WALLET, &[FRIEND], &[WALLET]),
            tx("dust1", 200, TWIN, &[WALLET], &[TWIN]),
            tx("dust2", 300, TWIN, &[WALLET], &[TWIN]),
        ];
        let finding = &scan(WALLET, &txs, 0).findings[0];
        assert_eq!(
            (finding.times_seen, finding.first_seen, finding.last_seen),
            (2, Some(200), Some(300))
        );
    }

    #[test]
    fn severity_needs_a_real_resemblance() {
        assert_eq!(severity(4, 4), Some("high"));
        assert_eq!(severity(3, 2), Some("medium"));
        assert_eq!(severity(0, 5), Some("low"));
        assert_eq!(severity(2, 2), None);
        assert_eq!(severity(4, 0), None);
    }

    #[test]
    fn parse_reads_signers_and_who_gained_sol_and_tokens() {
        let raw = json!({
            "blockTime": 1700000000,
            "meta": {
                "err": null,
                "preBalances": [1000, 0, 1],
                "postBalances": [800, 100, 1],
                "preTokenBalances": [{"owner": TWIN, "mint": "M", "uiTokenAmount": {"amount": "5"}}],
                "postTokenBalances": [
                    {"owner": TWIN, "mint": "M", "uiTokenAmount": {"amount": "4"}},
                    {"owner": WALLET, "mint": "M", "uiTokenAmount": {"amount": "1"}}
                ]
            },
            "transaction": {"message": {"accountKeys": [
                {"pubkey": TWIN, "signer": true},
                {"pubkey": WALLET, "signer": false},
                {"pubkey": "11111111111111111111111111111111", "signer": false}
            ]}}
        });
        let parsed = parse(WALLET, "sig", &raw).unwrap();
        assert_eq!(parsed.signers, vec![TWIN]);
        assert_eq!(parsed.gained, vec![WALLET]);
        assert_eq!(parsed.lost, vec![TWIN]);
        assert_eq!(parsed.wallet_lamports, 100);
    }

    #[test]
    fn failed_transactions_are_skipped() {
        let raw = json!({"meta": {"err": {"InstructionError": [0, "Custom"]}}});
        assert!(parse(WALLET, "sig", &raw).is_none());
    }
}
