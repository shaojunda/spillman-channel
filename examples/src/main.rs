use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod signer;
mod storage;
mod tx_builder;
mod utils;

#[derive(Parser)]
#[command(name = "spillman-cli")]
#[command(about = "Spillman Channel CLI - 单向支付通道管理工具", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 准备通道 - 创建 funding 和 refund 交易
    SetUp {
        /// 配置文件路径
        #[arg(long, default_value = "config.toml")]
        config: String,

        /// 输出目录（默认为当前目录）
        #[arg(long, default_value = ".")]
        output_dir: String,

        /// 商户地址（可选，覆盖配置文件中的商户地址）
        #[arg(long)]
        merchant_address: Option<String>,

        /// 通道容量（CKB，可选，覆盖配置文件）
        #[arg(long)]
        capacity: Option<u64>,

        /// 超时 epoch（可选，覆盖配置文件）
        #[arg(long)]
        timeout_epochs: Option<u64>,

        /// 是否使用 co-fund 模式（User + Merchant 共同出资）
        #[arg(long, default_value = "false")]
        co_fund: bool,

        /// 使用 funding_v2 实现（新版本）
        #[arg(long, default_value = "false")]
        use_v2: bool,
    },

    /// 签名交易
    SignTx {
        /// 交易文件路径
        #[arg(long)]
        tx_file: String,

        /// 私钥文件路径
        #[arg(long)]
        privkey_path: String,

        /// 是否为商户签名
        #[arg(long, default_value = "false")]
        is_merchant: bool,
    },

    /// 创建链下支付（commitment transaction）
    Pay {
        /// 支付金额（CKB）
        #[arg(long)]
        amount: u64,

        /// 通道信息文件路径（包含 Spillman Lock cell 信息）
        #[arg(long, default_value = "secrets/channel_info.json")]
        channel_file: String,

        /// 用户私钥文件路径
        #[arg(long)]
        privkey_path: String,

        /// 配置文件路径
        #[arg(long, default_value = "config.toml")]
        config: String,
    },

    /// 商户结算 commitment transaction
    Settle {
        /// Commitment transaction 文件路径
        #[arg(long)]
        tx_file: String,

        /// 商户私钥文件路径
        #[arg(long)]
        privkey_path: String,

        /// 配置文件路径
        #[arg(long, default_value = "config.toml")]
        config: String,
    },

    /// 用户退款（超时后）
    Refund {
        /// Funding transaction 文件路径
        #[arg(long)]
        tx_file: String,

        /// 配置文件路径
        #[arg(long, default_value = "config.toml")]
        config: String,

        /// 使用 refund_v2 实现（新版本）
        #[arg(long, default_value = "false")]
        use_v2: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::SetUp {
            config,
            output_dir,
            merchant_address,
            capacity,
            timeout_epochs,
            co_fund,
            use_v2,
        } => {
            if use_v2 {
                // Use v2 implementation (funding_v2)
                commands::setup::execute_v2(
                    &config,
                    &output_dir,
                    merchant_address.as_deref(),
                    capacity,
                    timeout_epochs,
                    co_fund,
                )
                .await?;
            } else {
                // Use v1 implementation (original funding)
                commands::setup::execute(
                    &config,
                    &output_dir,
                    merchant_address.as_deref(),
                    capacity,
                    timeout_epochs,
                    co_fund,
                )
                .await?;
            }
        }
        Commands::SignTx {
            tx_file,
            privkey_path,
            is_merchant,
        } => {
            commands::sign::execute(&tx_file, &privkey_path, is_merchant).await?;
        }
        Commands::Pay {
            amount,
            channel_file,
            privkey_path,
            config,
        } => {
            commands::pay::execute(&amount, &channel_file, &privkey_path, &config).await?;
        }
        Commands::Settle {
            tx_file,
            privkey_path,
            config,
        } => {
            commands::settle::execute(&tx_file, &privkey_path, &config).await?;
        }
        Commands::Refund {
            tx_file,
            config,
            use_v2,
        } => {
            if use_v2 {
                // Use v2 implementation (refund_v2)
                commands::refund::execute_v2(&tx_file, &config).await?;
            } else {
                // Use v1 implementation (original refund)
                commands::refund::execute(&tx_file, &config).await?;
            }
        }
    }

    Ok(())
}
