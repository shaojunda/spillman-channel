use anyhow::Result;
use ckb_sdk::rpc::CkbRpcClient;

/// Get current epoch from CKB node
pub async fn get_current_epoch(rpc_client: &CkbRpcClient) -> Result<u64> {
    let epoch = rpc_client.get_current_epoch()?;
    Ok(epoch.number.into())
}
