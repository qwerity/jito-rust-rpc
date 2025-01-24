use anyhow::{Result, anyhow};
use jito_sdk_rust::JitoJsonRpcSDK;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
    instruction::{Instruction, AccountMeta},
};
use std::str::FromStr;
use std::fs::File;
use std::io::BufReader;
use base64::{prelude::BASE64_STANDARD, Engine};
use serde_json::json;
use bs58;
use solana_sdk::hash::Hash;
use solana_transaction_status::UiTransactionEncoding;
use tokio::time::{sleep, Duration};

#[derive(Debug)]
struct BundleStatus {
    confirmation_status: Option<String>,
    err: Option<serde_json::Value>,
    transactions: Option<Vec<String>>,
}

fn load_keypair(path: &str) -> Result<Keypair> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let wallet: Vec<u8> = serde_json::from_reader(reader)?;
    Ok(Keypair::from_bytes(&wallet)?)
}

async fn prepare_serialized_transaction(sender: &Keypair, receiver: &Pubkey, jito_tip_account: &Pubkey, encoding: &str, recent_blockhash: Hash) -> Result<String>
{
    // Define amounts to send (in lamports)
    let main_transfer_amount = 1_000; // 0.000001 SOL
    let jito_tip_amount = 1_000; // 0.000001 SOL

    // Create instructions
    let main_transfer_ix = system_instruction::transfer(
        &sender.pubkey(),
        receiver,
        main_transfer_amount,
    );
    let jito_tip_ix = system_instruction::transfer(
        &sender.pubkey(),
        jito_tip_account,
        jito_tip_amount,
    );

    // Create memo instruction
    let memo_program_id = Pubkey::from_str("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr")?;
    let memo_ix = Instruction::new_with_bytes(
        memo_program_id,
        format!("hello world jito bundle {}", encoding).as_bytes(),
        vec![AccountMeta::new(sender.pubkey(), true)],
    );

    // Create a transaction
    let mut transaction = Transaction::new_with_payer(
        &[main_transfer_ix, memo_ix, jito_tip_ix],
        Some(&sender.pubkey()),
    );

    transaction.sign(&[&sender], recent_blockhash);

    // Serialize the transaction
    match encoding {
        "base58" => Ok(bs58::encode(bincode::serialize(&transaction)?).into_string()),
        "base64" => Ok(BASE64_STANDARD.encode(bincode::serialize(&transaction)?)),
        _ => Err(anyhow!("Invalid encoding: expected 'base58' or 'base64'")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up Solana RPC client (for getting recent blockhash and confirming transaction)
    let solana_rpc = RpcClient::new("https://api.mainnet-beta.solana.com".to_string());

    // Setup client Jito Block Engine endpoint
    let jito_sdk = JitoJsonRpcSDK::new("https://mainnet.block-engine.jito.wtf/api/v1", None);

    // Setup client Jito Block Engine endpoint with UUID
    //let jito_sdk = JitoJsonRpcSDK::new("https://mainnet.block-engine.jito.wtf/api/v1", "UUID-API-KEY");

    // Load the sender's keypair
    let sender = load_keypair("/path/to/wallet.json" )?;
    println!("Sender pubkey: {}", sender.pubkey());

    // Set up receiver and Jito tip account
    let receiver = Pubkey::from_str("YOUR_RECIEVER_PUBKEY")?;
    let random_tip_account = jito_sdk.get_random_tip_account().await?;
    let jito_tip_account = Pubkey::from_str(&random_tip_account)?;

    // Get recent blockhash
    let recent_blockhash = solana_rpc.get_latest_blockhash()?;

    // Serialize the transaction
    let serialized_base58_tx = prepare_serialized_transaction(&sender, &receiver, &jito_tip_account, "base58", recent_blockhash).await?;
    let serialized_base64_tx = prepare_serialized_transaction(&sender, &receiver, &jito_tip_account, "base64", recent_blockhash).await?;

    // Prepare bundle for submission (array of transactions)
    let bundle_base58 = json!([serialized_base58_tx]);
    let bundle_base64 = json!([serialized_base64_tx]);

    // UUID for the bundle
    let uuid = None;

    // Send bundle (base58) using Jito SDK
     println!("Sending bundle with 1 transaction...");
     let response_base58 = jito_sdk.send_bundle(Some(bundle_base58), UiTransactionEncoding::Base58.to_string(), uuid).await?;

     // Extract bundle UUID from response
     let bundle_base58_uuid = response_base58["result"]
         .as_str()
         .ok_or_else(|| anyhow!("Failed to get bundle (base58) UUID from response"))?;
     println!("Bundle (base58) sent with UUID: {}", bundle_base58_uuid);

    // Send bundle (base64) using Jito SDK
    println!("Sending bundle with 1 transaction...");
    let response_base64 = jito_sdk.send_bundle(Some(bundle_base64), UiTransactionEncoding::Base64.to_string(), uuid).await?;

     // Extract bundle UUID from response
     let bundle_base64_uuid = response_base64["result"]
         .as_str()
         .ok_or_else(|| anyhow!("Failed to get bundle (base64) UUID from response"))?;
     println!("Bundle (base64) sent with UUID: {}", bundle_base64_uuid);

     // Confirm bundle status
     let max_retries = 10;
     let retry_delay = Duration::from_secs(2);

     for attempt in 1..=max_retries {
         println!("Checking bundle status (attempt {}/{})", attempt, max_retries);

         let status_response = jito_sdk.get_in_flight_bundle_statuses(vec![bundle_base64_uuid.to_string()]).await?;

         if let Some(result) = status_response.get("result") {
             if let Some(value) = result.get("value") {
                 if let Some(statuses) = value.as_array() {
                     if let Some(bundle_status) = statuses.first() {
                         if let Some(status) = bundle_status.get("status") {
                             match status.as_str() {
                                 Some("Landed") => {
                                     println!("Bundle landed on-chain. Checking final status...");
                                     return check_final_bundle_status(&jito_sdk, bundle_base64_uuid).await;
                                 },
                                 Some("Pending") => {
                                     println!("Bundle is pending. Waiting...");
                                 },
                                 Some(status) => {
                                     println!("Unexpected bundle status: {}. Waiting...", status);
                                 },
                                 None => {
                                     println!("Unable to parse bundle status. Waiting...");
                                 }
                             }
                         } else {
                             println!("Status field not found in bundle status. Waiting...");
                         }
                     } else {
                         println!("Bundle status not found. Waiting...");
                     }
                 } else {
                     println!("Unexpected value format. Waiting...");
                 }
             } else {
                 println!("Value field not found in result. Waiting...");

             }
         } else if let Some(error) = status_response.get("error") {
             println!("Error checking bundle status: {:?}", error);
         } else {
             println!("Unexpected response format. Waiting...");
         }

         if attempt < max_retries {
             sleep(retry_delay).await;
         }
     }

     Err(anyhow!("Failed to confirm bundle status after {} attempts", max_retries))
 }

 async fn check_final_bundle_status(jito_sdk: &JitoJsonRpcSDK, bundle_uuid: &str) -> Result<()> {
    let max_retries = 10;
    let retry_delay = Duration::from_secs(2);

    for attempt in 1..=max_retries {
        println!("Checking final bundle status (attempt {}/{})", attempt, max_retries);

        let status_response = jito_sdk.get_bundle_statuses(vec![bundle_uuid.to_string()]).await?;
        let bundle_status = get_bundle_status(&status_response)?;

        match bundle_status.confirmation_status.as_deref() {
            Some("confirmed") => {
                println!("Bundle confirmed on-chain. Waiting for finalization...");
                check_transaction_error(&bundle_status)?;
            },
            Some("finalized") => {
                println!("Bundle finalized on-chain successfully!");
                check_transaction_error(&bundle_status)?;
                print_transaction_url(&bundle_status);
                return Ok(());
            },
            Some(status) => {
                println!("Unexpected final bundle status: {}. Continuing to poll...", status);
            },
            None => {
                println!("Unable to parse final bundle status. Continuing to poll...");
            }
        }

        if attempt < max_retries {
            sleep(retry_delay).await;
        }
    }

    Err(anyhow!("Failed to get finalized status after {} attempts", max_retries))
}

fn get_bundle_status(status_response: &serde_json::Value) -> Result<BundleStatus> {
    status_response
        .get("result")
        .and_then(|result| result.get("value"))
        .and_then(|value| value.as_array())
        .and_then(|statuses| statuses.get(0))
        .ok_or_else(|| anyhow!("Failed to parse bundle status"))
        .map(|bundle_status| BundleStatus {
            confirmation_status: bundle_status.get("confirmation_status").and_then(|s| s.as_str()).map(String::from),
            err: bundle_status.get("err").cloned(),
            transactions: bundle_status.get("transactions").and_then(|t| t.as_array()).map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
            }),
        })
}

fn check_transaction_error(bundle_status: &BundleStatus) -> Result<()> {
    if let Some(err) = &bundle_status.err {
        if err["Ok"].is_null() {
            println!("Transaction executed without errors.");
            Ok(())
        } else {
            println!("Transaction encountered an error: {:?}", err);
            Err(anyhow!("Transaction encountered an error"))
        }
    } else {
        Ok(())
    }
}

fn print_transaction_url(bundle_status: &BundleStatus) {
    if let Some(transactions) = &bundle_status.transactions {
        if let Some(tx_id) = transactions.first() {
            println!("Transaction URL: https://solscan.io/tx/{}", tx_id);
        } else {
            println!("Unable to extract transaction ID.");
        }
    } else {
        println!("No transactions found in the bundle status.");
    }
}
