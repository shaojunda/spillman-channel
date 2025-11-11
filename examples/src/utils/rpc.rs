use anyhow::Result;
use ckb_sdk::rpc::CkbRpcClient;

/// Get current epoch from CKB node
pub async fn get_current_epoch(rpc_client: &CkbRpcClient) -> Result<u64> {
    let epoch = rpc_client.get_current_epoch()?;
    Ok(epoch.number.into())
}

/// Get current timestamp from CKB node's latest block
/// Returns Unix timestamp in seconds
pub async fn get_current_timestamp(rpc_client: &CkbRpcClient) -> Result<u64> {
    let tip_header = rpc_client.get_tip_header()?;
    let timestamp: u64 = tip_header.inner.timestamp.into();
    // CKB timestamp is in milliseconds, convert to seconds
    Ok(timestamp / 1000)
}
