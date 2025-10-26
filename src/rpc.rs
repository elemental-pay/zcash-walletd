use crate::account::AccountBalance;
use crate::db::Db;
use crate::network::Network;
use crate::transaction::Transfer;
use rocket::response::Debug;
use rocket::serde::{json::Json, Deserialize, Serialize};
use rocket::State;
use sapling_crypto::keys::PreparedIncomingViewingKey;
use sapling_crypto::zip32::ExtendedFullViewingKey;
use crate::scan::{scan_blocks, scan_transaction, get_latest_height};
use crate::{FVK, from_tonic, WalletConfig};
use tokio_stream::StreamExt;
use crate::lwd_rpc::compact_tx_streamer_client::CompactTxStreamerClient;
use tonic::Request;
use crate::lwd_rpc::*;
use crate::scan::{ScannerOutput, ScanError};
use crate::WalletMap;
use zcash_client_backend::encoding::decode_extended_full_viewing_key;
use zcash_protocol::consensus::NetworkConstants;
use std::fs;
use std::path::Path;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct CreateAccountRequest {
    wallet_id: String,
    label: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CreateWalletRequest {
    wallet_id: String,
    vk: String,
}

#[derive(Serialize, Deserialize)]
pub struct CreateWalletResponse {
    wallet_id: String,
}


#[derive(Serialize, Deserialize)]
pub struct CreateAccountResponse {
    account_index: u32,
    address: String,
}

#[post("/create_wallet", data = "<request>")]
pub fn create_wallet(
    request: Json<CreateWalletRequest>, state: &State<WalletMap>, config: &State<WalletConfig>
) -> Result<Json<CreateWalletResponse>, Debug<anyhow::Error>>  {
    let request = request.into_inner();
    let wallet_id = request.wallet_id;
    if {
        let map = state.read().unwrap();
        map.get(&wallet_id).is_some()
    } {
        return Err(Debug(anyhow::anyhow!("Wallet `{}` already exists", wallet_id)));
    }
    let vk = request.vk;
    // let cfg_path = format!("./{}/config.json", wallet_id);
    let dir_path = format!("./{}", wallet_id);
    let db_path = format!("{}/wallet.db", dir_path);
    let network = config.network();

    fs::create_dir_all(&dir_path).expect("Invalid db path");

    let fvk = decode_extended_full_viewing_key(
        network.hrp_sapling_extended_full_viewing_key(),
        &vk,
    ).expect("Invalid viewing key");

    let db = Arc::new(Db::new(network, &db_path, &fvk));

    let db_exists = db.create().unwrap();
    if !db_exists {
        db.new_account("")?;
    }

    let mut map = state.write().unwrap();
    map.insert(wallet_id.to_string(), db);

    let rep = CreateWalletResponse {
        wallet_id: wallet_id,
    };
    Ok(Json(rep))
}

#[post("/create_account", data = "<request>")]
pub fn create_account(
    request: Json<CreateAccountRequest>,
    state: &State<WalletMap>
    // db: &State<Db>,
) -> Result<Json<CreateAccountResponse>, Debug<anyhow::Error>> {
    let request = request.into_inner();
    let name = request.label.unwrap_or("".to_string());
    let wallet_id = request.wallet_id;
    let db = {
        let map = state.read().unwrap();
        map.get(&wallet_id)
            .cloned()
            .ok_or_else(|| Debug(anyhow::anyhow!("Wallet `{}` does not exist", wallet_id)))?
    };

    let account = db.new_account(&name)?;
    let rep = CreateAccountResponse {
        account_index: account.account_index,
        address: account.address,
    };

    Ok(Json(rep))
}
#[derive(Serialize, Deserialize)]
pub struct CreateAddressRequest {
    account_index: u32,
    label: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CreateAddressResponse {
    address: String,
    address_index: u32,
}

#[post("/create_address", data = "<request>")]
pub fn create_address(
    request: Json<CreateAddressRequest>,
    db: &State<Db>,
) -> Result<Json<CreateAddressResponse>, Debug<anyhow::Error>> {
    let request = request.into_inner();
    let name = request.label.unwrap_or("".to_string());
    let sub_account = db.new_sub_account(request.account_index, &name)?;

    let rep = CreateAddressResponse {
        address: sub_account.address.clone(),
        address_index: sub_account.sub_account_index,
    };
    Ok(Json(rep))
}
#[derive(Serialize, Deserialize)]
pub struct GetAccountsRequest {
    tag: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct GetAccountsResponse {
    subaddress_accounts: Vec<AccountBalance>,
    total_balance: u64,
    total_unlocked_balance: u64,
}

#[post("/get_accounts", data = "<_request>")]
pub async fn get_accounts(
    _request: Json<GetAccountsRequest>,
    db: &State<Db>,
    config: &State<WalletConfig>,
) -> Result<Json<GetAccountsResponse>, Debug<anyhow::Error>> {
    let mut client = CompactTxStreamerClient::connect(config.lwd_url.clone()).await.map_err(from_tonic)?;
    let latest_height = get_latest_height(&mut client).await?;
    let sub_accounts = db.get_accounts(latest_height, config.confirmations)?;
    let total_balance: u64 = sub_accounts.iter().map(|sa| sa.balance).sum();
    let total_unlocked_balance: u64 = sub_accounts.iter().map(|sa| sa.unlocked_balance).sum();

    let rep = GetAccountsResponse {
        subaddress_accounts: sub_accounts,
        total_balance,
        total_unlocked_balance,
    };
    Ok(Json(rep))
}

#[derive(Serialize, Deserialize)]
pub struct GetTransactionByIdRequest {
    pub txid: String,
    pub account_index: u32,
}

#[derive(Serialize, Deserialize)]
pub struct GetTransactionByIdResponse {
    pub transfer: Transfer,
    pub transfers: Vec<Transfer>,
}

#[post("/get_transfer_by_txid", data = "<request>")]
pub async fn get_transaction(
    request: Json<GetTransactionByIdRequest>,
    db: &State<Db>,
    config: &State<WalletConfig>,
) -> Result<Json<GetTransactionByIdResponse>, Debug<anyhow::Error>> {
    let request = request.into_inner();
    let mut client = CompactTxStreamerClient::connect(config.lwd_url.clone()).await.map_err(from_tonic)?;
    let latest_height = get_latest_height(&mut client).await?;
    let transfers = db.get_transfers_by_txid(latest_height, &request.txid, request.account_index, config.confirmations)?;
    let rep = GetTransactionByIdResponse {
        transfer: transfers[0].clone(),
        transfers,
    };
    Ok(Json(rep))
}

#[derive(Serialize, Deserialize)]
pub struct GetTransfersRequest {
    pub account_index: u32,
    pub r#in: bool,
    pub subaddr_indices: Vec<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct GetTransfersResponse {
    pub r#in: Vec<Transfer>,
}

#[post("/get_transfers", data = "<request>")]
pub async fn get_transfers(
    request: Json<GetTransfersRequest>,
    db: &State<Db>,
    config: &State<WalletConfig>,
) -> Result<Json<GetTransfersResponse>, Debug<anyhow::Error>> {
    let request = request.into_inner();
    assert!(request.r#in);
    let mut client = CompactTxStreamerClient::connect(config.lwd_url.clone()).await.map_err(from_tonic)?;
    let latest_height = get_latest_height(&mut client).await?;
    let transfers = db.get_transfers(latest_height, request.account_index, &request.subaddr_indices, config.confirmations)?;
    let rep = GetTransfersResponse {
        r#in: transfers,
    };
    Ok(Json(rep))
}

#[derive(Serialize, Deserialize)]
pub struct GetFeeEstimateRequest {
}

#[derive(Serialize, Deserialize)]
pub struct GetFeeEstimateResponse {
    pub fee: u64,
}

// Roughly estimate at 2 transparent in/out + 2 shielded in/out
// We cannot implement ZIP-321 here because we don't have
// the transaction
const LOGICAL_ACTION_FEE: u64 = 5000u64;

#[post("/get_fee_estimate", data = "<_request>")]
pub fn get_fee_estimate(_request: Json<GetFeeEstimateRequest>) -> Result<Json<GetFeeEstimateResponse>, Debug<anyhow::Error>> {
    let rep = GetFeeEstimateResponse {
        fee: 4 * LOGICAL_ACTION_FEE,
    };
    Ok(Json(rep))
}

#[derive(Serialize, Deserialize)]
pub struct GetHeightRequest {
}

#[derive(Serialize, Deserialize)]
pub struct GetHeightResponse {
    pub height: u32,
}

#[post("/get_height", data = "<_request>")]
pub async fn get_height(_request: Json<GetHeightRequest>, config: &State<WalletConfig>) -> Result<Json<GetHeightResponse>, Debug<anyhow::Error>> {
    let mut client = CompactTxStreamerClient::connect(config.lwd_url.clone()).await.map_err(from_tonic)?;
    let latest_height = get_latest_height(&mut client).await?;
    let rep = GetHeightResponse {
        height: latest_height
    };
    Ok(Json(rep))
}

#[derive(Serialize, Deserialize)]
pub struct SyncInfoRequest {
}

#[derive(Serialize, Deserialize)]
pub struct SyncInfoResponse {
    pub target_height: u32,
    pub height: u32,
}

#[post("/sync_info", data = "<_request>")]
pub async fn sync_info(_request: Json<SyncInfoRequest>, config: &State<WalletConfig>) -> Result<Json<SyncInfoResponse>, Debug<anyhow::Error>> {
    let mut client = CompactTxStreamerClient::connect(config.lwd_url.clone()).await.map_err(from_tonic)?;
    let rep = client.get_lightd_info(Request::new(Empty {})).await.map_err(from_tonic)?.into_inner();
    let rep = SyncInfoResponse {
        target_height: rep.block_height as u32,
        height: rep.estimated_height as u32,
    };
    Ok(Json(rep))
}

#[derive(Serialize, Deserialize)]
pub struct ScanRequest {
    start_height: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct ScanResponse {}

#[post("/request_scan", data = "<request>")]
pub async fn request_scan(
    request: Json<ScanRequest>,
    db: &State<Db>,
    fvk: &State<FVK>,
    config: &State<WalletConfig>,
) -> Result<Json<ScanResponse>, Debug<anyhow::Error>> {
    let network = config.network();
    let request = request.into_inner();
    let fvk: ExtendedFullViewingKey = fvk.0.lock().unwrap().clone();

    let res = scan(network, fvk, request.start_height, db, config).await;
    if let Err(error) = res { // Rewind if we hit a chain reorg but don't error
        match error.root_cause().downcast_ref::<ScanError>() {
            Some(ScanError::Reorganization) => {
                let synced_height = db.get_synced_height()?;
                db.truncate_height(synced_height - config.confirmations)
            },
            None => Err(error),
        }?
    }
    let rep = ScanResponse {};
    Ok(Json(rep))
}

pub async fn scan(network: Network, fvk: ExtendedFullViewingKey, start_height: Option<u32>, db: &State<Db>, config: &State<WalletConfig>) -> anyhow::Result<()> {
    let vk = fvk.fvk.vk.clone();
    let ivk = vk.ivk();
    let pivk = PreparedIncomingViewingKey::new(&ivk);

    let start_height = match start_height {
        Some(h) => h,
        None => db.get_synced_height()? + 1,
    };

    db.truncate_height(start_height)?;
    let prev_block_hash = db.get_block_hash(start_height - 1)?;
    let mut client = CompactTxStreamerClient::connect(config.lwd_url.clone()).await.map_err(from_tonic)?;
    let (mut tx_stream, scanner_handle) = scan_blocks(network.clone(), start_height, &config.lwd_url, &fvk, prev_block_hash).await?;
    let mut nf_map = db.get_nfs()?;
    while let Some(scan_output) = tx_stream.next().await {
        match scan_output {
            ScannerOutput::TxIndex(tx_index) => {
                let (spends, outputs, value) = scan_transaction(&network, &mut client, tx_index.height, tx_index.tx_id, tx_index.position, &vk, &pivk, &nf_map).await?;
                let id_tx = db.store_tx(tx_index.tx_id.as_ref(), tx_index.height, value)?;
                for id_note in spends.iter() {
                    db.mark_spent(*id_note, id_tx)?;
                }
                for n in outputs.iter() {
                    let id_note = db.store_note(n, id_tx)?;
                    nf_map.insert(n.nf, id_note);
                }
                notify_tx(tx_index.tx_id.as_ref(), &config.notify_tx_url).await?;
            }
            ScannerOutput::Block(block) => {
                db.store_block(block.height, &block.hash)?;
            }
        }
    }
    scanner_handle.await?;

    Ok(())
}

pub async fn notify_tx(tx_id: &[u8], notify_tx_url: &str) -> anyhow::Result<()> {
    let mut tx_id = tx_id.to_vec();
    tx_id.reverse();
    let url = notify_tx_url.to_string() + &hex::encode(&tx_id);
    // TODO: Remove self signed certificate accept
    let res = reqwest::Client::builder().danger_accept_invalid_certs(true)
        .build()?.get(url).send().await;
    if let Err(e) = res {
        log::warn!("Failed to notify new tx: {}", e.to_string());
    }

    Ok(())
}
