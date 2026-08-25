//! Vendor-side license tooling. **Never shipped to customers** — this is an
//! example binary, not part of the library, and the signing key it uses must
//! stay off every customer machine and out of this repo.
//!
//! # Generate a keypair (once, then keep the secret safe)
//!
//! ```bash
//! cargo run -p one-billing --example license_tool -- keygen
//! ```
//!
//! Put the printed **public** key into `LICENSE_PUBLIC_KEY_B64`
//! (`src/license_key.rs`) and store the **secret** key in your password
//! manager. Rotating the public key invalidates every key already issued.
//!
//! # Issue a license
//!
//! ```bash
//! cargo run -p one-billing --example license_tool -- issue \
//!     --secret <SECRET_KEY_B64> \
//!     --customer "Acme Inc" \
//!     --tier enterprise \
//!     --seats 50 \
//!     --days 365 \
//!     --tenant-cap 10 \
//!     --agent-node-cap 20 \
//!     --cpu-cores-cap 64 \
//!     --memory-mb-cap 131072 \
//!     --module /admin/* \
//!     --serial SN-0001
//! ```
//!
//! Omit `--days` for a perpetual license; omit `--seats` (or any of the
//! `--*-cap` flags) to leave that quota unlimited. `--module` is repeatable —
//! each occurrence names one module this license authorizes, always
//! open-ended (activation window / per-module expiry aren't exposed here;
//! construct a `LicensePayload` directly for that). Omitting `--module`
//! entirely leaves the license with no per-module restriction at all — see
//! `LicensePayload::module_authorized`'s doc comment for why that means
//! "every module authorized", not "none".
//!
//! # Inspect a key (no secret needed)
//!
//! ```bash
//! cargo run -p one-billing --example license_tool -- inspect --key ONEWORK-...
//! ```

use dream_domain_billing::license_key::{LicenseModuleGrant, LicensePayload, sign_license_key, verify_license_key};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");

    match cmd {
        "keygen" => keygen(),
        "issue" => issue(&args),
        "inspect" => inspect(&args),
        _ => eprintln!(
            "usage:\n  license_tool keygen\n  license_tool issue --secret <B64> --customer <NAME> \
             --tier <free|team|enterprise> [--seats N] [--days N] [--tenant-cap N] [--agent-node-cap N] \
             [--cpu-cores-cap N] [--memory-mb-cap N] [--module NAME]... [--serial S] [--app-id ID] \
             [--file-name NAME]\n  license_tool inspect --key <KEY>"
        ),
    }
}

fn keygen() {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ed25519_dalek::SigningKey;

    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).expect("OS entropy source unavailable");
    let sk = SigningKey::from_bytes(&seed);

    println!("SECRET (keep private, never commit):");
    println!("  {}", URL_SAFE_NO_PAD.encode(sk.to_bytes()));
    println!();
    println!("PUBLIC (paste into LICENSE_PUBLIC_KEY_B64 in src/license_key.rs):");
    println!("  {}", URL_SAFE_NO_PAD.encode(sk.verifying_key().to_bytes()));
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let idx = args.iter().position(|a| a == name)?;
    args.get(idx + 1).map(String::as_str)
}

/// Every occurrence's value, in order — for `--module`, which is repeatable.
fn flag_all<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.iter()
        .zip(args.iter().skip(1))
        .filter(|(a, _)| *a == name)
        .map(|(_, v)| v.as_str())
        .collect()
}

fn issue(args: &[String]) {
    let Some(secret) = flag(args, "--secret") else {
        return eprintln!("--secret is required");
    };
    let Some(customer) = flag(args, "--customer") else {
        return eprintln!("--customer is required");
    };
    let tier = flag(args, "--tier").unwrap_or("team");
    if !matches!(tier, "free" | "team" | "enterprise") {
        return eprintln!("--tier must be free | team | enterprise");
    }
    let seats = flag(args, "--seats").and_then(|s| s.parse::<i64>().ok());
    let now = dream_core_common::now_ms();
    let exp = flag(args, "--days")
        .and_then(|s| s.parse::<i64>().ok())
        .map(|days| now + days * 24 * 3600 * 1000);

    let modules = flag_all(args, "--module")
        .into_iter()
        .map(|module| LicenseModuleGrant {
            module: module.to_owned(),
            starts_at: None,
            expires_at: None,
        })
        .collect();

    let payload = LicensePayload {
        lid: format!("lic_{}", uuid::Uuid::now_v7().simple()),
        customer: customer.to_owned(),
        tier: tier.to_owned(),
        seats,
        exp,
        iat: now,
        tenant_cap: flag(args, "--tenant-cap").and_then(|s| s.parse::<i64>().ok()),
        agent_node_cap: flag(args, "--agent-node-cap").and_then(|s| s.parse::<i64>().ok()),
        cpu_cores_cap: flag(args, "--cpu-cores-cap").and_then(|s| s.parse::<i64>().ok()),
        memory_mb_cap: flag(args, "--memory-mb-cap").and_then(|s| s.parse::<i64>().ok()),
        modules,
        serial: flag(args, "--serial").map(str::to_owned),
        app_id: flag(args, "--app-id").map(str::to_owned),
        file_name: flag(args, "--file-name").map(str::to_owned),
    };

    match sign_license_key(&payload, secret) {
        Ok(key) => {
            println!("customer : {}", payload.customer);
            println!("tier     : {}", payload.tier);
            println!(
                "seats    : {}",
                payload
                    .seats
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "tier default".into())
            );
            println!(
                "expires  : {}",
                payload.exp.map(|e| e.to_string()).unwrap_or_else(|| "never".into())
            );
            println!("license id: {}", payload.lid);
            if !payload.modules.is_empty() {
                let names: Vec<&str> = payload.modules.iter().map(|m| m.module.as_str()).collect();
                println!("modules  : {}", names.join(", "));
            }
            println!();
            println!("{key}");
        }
        Err(e) => eprintln!("failed to sign: {e}"),
    }
}

fn inspect(args: &[String]) {
    let Some(key) = flag(args, "--key") else {
        return eprintln!("--key is required");
    };
    match verify_license_key(key) {
        Ok(p) => println!("{}", serde_json::to_string_pretty(&p).unwrap()),
        Err(e) => eprintln!("invalid: {e}"),
    }
}
