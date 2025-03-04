use std::time::Instant;

use actix::Addr;
use ansi_term::Color::{Blue, Cyan, Green, White, Yellow};
use log::info;
use serde_json::json;
use sysinfo::{get_current_pid, Pid, ProcessExt, System, SystemExt};

use near_chain::Tip;
use near_network::types::{NetworkInfo, PeerId};
use near_primitives::serialize::to_base;
use near_telemetry::{telemetry, TelemetryActor};

use crate::types::{BlockProducer, ShardSyncStatus, SyncStatus};
use std::cmp::min;

/// A helper that prints information about current chain and reports to telemetry.
pub struct InfoHelper {
    /// Timestamp when client was started.
    started: Instant,
    /// Total number of blocks processed.
    num_blocks_processed: u64,
    /// Total number of transactions processed.
    num_tx_processed: u64,
    /// Process id to query resources.
    pid: Option<Pid>,
    /// System reference.
    sys: System,
    /// Sign telemetry with block producer key if available.
    block_producer: Option<BlockProducer>,
    /// Telemetry actor.
    telemetry_actor: Addr<TelemetryActor>,
}

impl InfoHelper {
    pub fn new(
        telemetry_actor: Addr<TelemetryActor>,
        block_producer: Option<BlockProducer>,
    ) -> Self {
        InfoHelper {
            started: Instant::now(),
            num_blocks_processed: 0,
            num_tx_processed: 0,
            pid: get_current_pid().ok(),
            sys: System::new(),
            telemetry_actor,
            block_producer,
        }
    }

    pub fn block_processed(&mut self, num_transactions: u64) {
        self.num_blocks_processed += 1;
        self.num_tx_processed += num_transactions;
    }

    pub fn info(
        &mut self,
        head: &Tip,
        sync_status: &SyncStatus,
        node_id: &PeerId,
        network_info: &NetworkInfo,
        is_validator: bool,
        num_validators: usize,
    ) {
        let (cpu_usage, memory) = if let Some(pid) = self.pid {
            if self.sys.refresh_process(pid) {
                let proc = self
                    .sys
                    .get_process(pid)
                    .expect("refresh_process succeeds, this should be not None");
                (proc.cpu_usage(), proc.memory())
            } else {
                (0.0, 0)
            }
        } else {
            (0.0, 0)
        };

        // Block#, Block Hash, is validator/# validators, active/max peers, traffic, blocks/sec & tx/sec
        let avg_bls = (self.num_blocks_processed as f64)
            / (self.started.elapsed().as_millis() as f64)
            * 1000.0;
        let avg_tps =
            (self.num_tx_processed as f64) / (self.started.elapsed().as_millis() as f64) * 1000.0;
        info!(target: "info", "{} {} {} {} {} {}",
              Yellow.bold().paint(display_sync_status(&sync_status, &head)),
              White.bold().paint(format!("{}/{}", if is_validator { "V" } else { "-" }, num_validators)),
              Cyan.bold().paint(format!("{:2}/{:?}/{:2} peers", network_info.num_active_peers, network_info.most_weight_peers.len(), network_info.peer_max_count)),
              Cyan.bold().paint(format!("⬇ {} ⬆ {}", pretty_bytes_per_sec(network_info.received_bytes_per_sec), pretty_bytes_per_sec(network_info.sent_bytes_per_sec))),
              Green.bold().paint(format!("{:.2} bls {:.2} tps", avg_bls, avg_tps)),
              Blue.bold().paint(format!("CPU: {:.0}%, Mem: {}", cpu_usage, pretty_bytes(memory * 1024)))
        );
        self.started = Instant::now();
        self.num_blocks_processed = 0;
        self.num_tx_processed = 0;

        telemetry(
            &self.telemetry_actor,
            try_sign_json(
                json!({
                    "account_id": self.block_producer.clone().map(|bp| bp.account_id).unwrap_or("".to_string()),
                    "is_validator": is_validator,
                    "node_id": format!("{}", node_id),
                    "status": display_sync_status(&sync_status, &head),
                    "latest_block_hash": to_base(&head.last_block_hash),
                    "latest_block_height": head.height,
                    "num_peers":  network_info.num_active_peers,
                    "bandwidth_download": network_info.received_bytes_per_sec,
                    "bandwidth_upload": network_info.sent_bytes_per_sec,
                    "cpu": cpu_usage,
                    "memory": memory,
                }),
                &self.block_producer,
            ),
        );
    }
}

/// Tries to sign given JSON with block producer if it's present and all succeeds.
fn try_sign_json(
    mut value: serde_json::Value,
    block_producer: &Option<BlockProducer>,
) -> serde_json::Value {
    let mut signature = "".to_string();
    if let Some(bp) = block_producer {
        if let Ok(s) = serde_json::to_string(&value) {
            signature = format!("{}", bp.signer.sign(s.as_bytes()));
        }
    }
    value["signature"] = signature.into();
    value
}

fn display_sync_status(sync_status: &SyncStatus, head: &Tip) -> String {
    match sync_status {
        SyncStatus::AwaitingPeers => format!("#{:>8} Waiting for peers", head.height),
        SyncStatus::NoSync => format!("#{:>8} {}", head.height, head.last_block_hash),
        SyncStatus::HeaderSync { current_height, highest_height } => {
            let percent = if *highest_height == 0 {
                0
            } else {
                min(current_height, highest_height) * 100 / highest_height
            };
            format!("#{:>8} Downloading headers {}%", head.height, percent)
        }
        SyncStatus::BodySync { current_height, highest_height } => {
            let percent =
                if *highest_height == 0 { 0 } else { current_height * 100 / highest_height };
            format!("#{:>8} Downloading blocks {}%", current_height, percent)
        }
        SyncStatus::StateSync(_sync_hash, shard_statuses) => {
            let mut res = String::from("State ");
            for (shard_id, shard_status) in shard_statuses {
                res = res
                    + format!(
                        "{}: {}",
                        shard_id,
                        match shard_status {
                            ShardSyncStatus::StateDownload {
                                start_time: _,
                                prev_update_time: _,
                                prev_downloaded_size: _,
                                downloaded_size: _,
                                total_size: _,
                            } => format!("download"),
                            ShardSyncStatus::StateValidation => format!("validation"),
                            ShardSyncStatus::StateDone => format!("done"),
                            ShardSyncStatus::Error(error) => format!("error {}", error),
                        }
                    )
                    .as_str();
            }
            res
        }
        SyncStatus::StateSyncDone => format!("State sync done"),
    }
}

const KILOBYTE: u64 = 1024;
const MEGABYTE: u64 = KILOBYTE * 1024;
const GIGABYTE: u64 = MEGABYTE * 1024;

/// Format bytes per second in a nice way.
fn pretty_bytes_per_sec(num: u64) -> String {
    if num < 100 {
        // Under 0.1 kiB, display in bytes.
        format!("{} B/s", num)
    } else if num < MEGABYTE {
        // Under 1.0 MiB/sec display in kiB/sec.
        format!("{:.1}kiB/s", num as f64 / KILOBYTE as f64)
    } else {
        format!("{:.1}MiB/s", num as f64 / MEGABYTE as f64)
    }
}

fn pretty_bytes(num: u64) -> String {
    if num < 1024 {
        format!("{} B", num)
    } else if num < MEGABYTE {
        format!("{:.1} kiB", num as f64 / KILOBYTE as f64)
    } else if num < GIGABYTE {
        format!("{:.1} MiB", num as f64 / MEGABYTE as f64)
    } else {
        format!("{:.1} GiB", num as f64 / GIGABYTE as f64)
    }
}
