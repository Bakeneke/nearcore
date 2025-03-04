use std::cmp;
use std::collections::{HashMap, HashSet};

use actix::Recipient;
use chrono::{DateTime, Duration, Utc};
use log::{debug, error, info};
use rand::{thread_rng, Rng};

use near_chain::{Chain, Tip};
use near_network::types::ReasonForBan;
use near_network::{FullPeerInfo, NetworkRequests};
use near_primitives::hash::CryptoHash;
use near_primitives::types::{BlockIndex, ShardId};

use crate::types::{ShardSyncStatus, SyncStatus};

/// Maximum number of block headers send over the network.
pub const MAX_BLOCK_HEADERS: u64 = 512;

const BLOCK_HEADER_PROGRESS_TIMEOUT: i64 = 2;

/// Maximum number of block header hashes to send as part of a locator.
pub const MAX_BLOCK_HEADER_HASHES: usize = 20;

/// Maximum number of blocks to request in one step.
const MAX_BLOCK_REQUEST: usize = 100;

/// Maximum number of blocks to ask from single peer.
const MAX_PEER_BLOCK_REQUEST: usize = 10;

const BLOCK_REQUEST_TIMEOUT: i64 = 6;
const BLOCK_SOME_RECEIVED_TIMEOUT: i64 = 1;
const BLOCK_REQUEST_BROADCAST_OFFSET: u64 = 2;

/// Sync state download timeout in minutes.
const STATE_SYNC_TIMEOUT: i64 = 10;

/// Adapter to allow to test Header/Body/State sync without actix.
pub trait SyncNetworkAdapter: Sync + Send {
    fn send(&self, msg: NetworkRequests);
}

pub struct SyncNetworkRecipient {
    network_recipient: Recipient<NetworkRequests>,
}

unsafe impl Sync for SyncNetworkRecipient {}

impl SyncNetworkRecipient {
    pub fn new(network_recipient: Recipient<NetworkRequests>) -> Box<Self> {
        Box::new(Self { network_recipient })
    }
}

impl SyncNetworkAdapter for SyncNetworkRecipient {
    fn send(&self, msg: NetworkRequests) {
        let _ = self.network_recipient.do_send(msg);
    }
}

/// Get random peer from the most weighted peers.
pub fn most_weight_peer(most_weight_peers: &Vec<FullPeerInfo>) -> Option<FullPeerInfo> {
    if most_weight_peers.len() == 0 {
        return None;
    }
    let index = thread_rng().gen_range(0, most_weight_peers.len());
    Some(most_weight_peers[index].clone())
}

/// Helper to keep track of sync headers.
/// Handles major re-orgs by finding closest header that matches and re-downloading headers from that point.
pub struct HeaderSync {
    network_adapter: Box<dyn SyncNetworkAdapter>,
    history_locator: Vec<(BlockIndex, CryptoHash)>,
    prev_header_sync: (DateTime<Utc>, BlockIndex, BlockIndex),
    syncing_peer: Option<FullPeerInfo>,
    stalling_ts: Option<DateTime<Utc>>,
}

impl HeaderSync {
    pub fn new(network_adapter: Box<dyn SyncNetworkAdapter>) -> Self {
        HeaderSync {
            network_adapter,
            history_locator: vec![],
            prev_header_sync: (Utc::now(), 0, 0),
            syncing_peer: None,
            stalling_ts: None,
        }
    }

    pub fn run(
        &mut self,
        sync_status: &mut SyncStatus,
        chain: &mut Chain,
        highest_height: BlockIndex,
        most_weight_peers: &Vec<FullPeerInfo>,
    ) -> Result<(), near_chain::Error> {
        let header_head = chain.header_head()?;
        if !self.header_sync_due(sync_status, &header_head) {
            return Ok(());
        }

        let enable_header_sync = match sync_status {
            SyncStatus::HeaderSync { .. }
            | SyncStatus::BodySync { .. }
            | SyncStatus::StateSyncDone => true,
            SyncStatus::NoSync | SyncStatus::AwaitingPeers => {
                let sync_head = chain.sync_head()?;
                debug!(target: "sync", "Sync: initial transition to Header sync. Sync head: {} at {}, resetting to {} at {}",
                    sync_head.last_block_hash, sync_head.height,
                    header_head.last_block_hash, header_head.height,
                );
                // Reset sync_head to header_head on initial transition to HeaderSync.
                chain.reset_sync_head()?;
                self.history_locator.retain(|&x| x.0 == 0);
                true
            }
            _ => false,
        };

        if enable_header_sync {
            *sync_status =
                SyncStatus::HeaderSync { current_height: header_head.height, highest_height };
            let header_head = chain.header_head()?;
            self.syncing_peer = None;
            if let Some(peer) = most_weight_peer(&most_weight_peers) {
                if peer.chain_info.total_weight > header_head.total_weight {
                    self.syncing_peer = self.request_headers(chain, peer);
                }
            }
        }
        Ok(())
    }

    fn header_sync_due(&mut self, sync_status: &SyncStatus, header_head: &Tip) -> bool {
        let now = Utc::now();
        let (timeout, latest_height, prev_height) = self.prev_header_sync;

        // Received all necessary header, can request more.
        let all_headers_received = header_head.height >= prev_height + MAX_BLOCK_HEADERS - 4;
        // No headers processed and it's past timeout, request more.
        let stalling = header_head.height <= latest_height && now > timeout;

        // Always enable header sync on initial state transition from NoSync / AwaitingPeers.
        let force_sync = match sync_status {
            SyncStatus::NoSync | SyncStatus::AwaitingPeers => true,
            _ => false,
        };

        if force_sync || all_headers_received || stalling {
            self.prev_header_sync =
                (now + Duration::seconds(10), header_head.height, header_head.height);

            if stalling {
                if self.stalling_ts.is_none() {
                    self.stalling_ts = Some(now);
                } else {
                    self.stalling_ts = None;
                }
            }

            if all_headers_received {
                self.stalling_ts = None;
            } else {
                if let Some(ref stalling_ts) = self.stalling_ts {
                    if let Some(ref peer) = self.syncing_peer {
                        match sync_status {
                            SyncStatus::HeaderSync { highest_height, .. } => {
                                if now > *stalling_ts + Duration::seconds(120)
                                    && *highest_height == peer.chain_info.height
                                {
                                    info!(target: "sync", "Sync: ban a fraudulent peer: {}, claimed height: {}, total weight: {}",
                                        peer.peer_info, peer.chain_info.height, peer.chain_info.total_weight);
                                    self.network_adapter.send(NetworkRequests::BanPeer {
                                        peer_id: peer.peer_info.id.clone(),
                                        ban_reason: ReasonForBan::HeightFraud,
                                    });
                                }
                            }
                            _ => (),
                        }
                    }
                }
            }
            self.syncing_peer = None;
            true
        } else {
            // Resetting the timeout as long as we make progress.
            if header_head.height > latest_height {
                self.prev_header_sync = (
                    now + Duration::seconds(BLOCK_HEADER_PROGRESS_TIMEOUT),
                    header_head.height,
                    prev_height,
                );
            }
            false
        }
    }

    /// Request headers from a given peer to advance the chain.
    fn request_headers(&mut self, chain: &mut Chain, peer: FullPeerInfo) -> Option<FullPeerInfo> {
        if let Ok(locator) = self.get_locator(chain) {
            debug!(target: "sync", "Sync: request headers: asking {} for headers, {:?}", peer.peer_info.id, locator);
            self.network_adapter.send(NetworkRequests::BlockHeadersRequest {
                hashes: locator,
                peer_id: peer.peer_info.id.clone(),
            });
            return Some(peer);
        }
        None
    }

    fn get_locator(&mut self, chain: &mut Chain) -> Result<Vec<CryptoHash>, near_chain::Error> {
        let tip = chain.sync_head()?;
        let heights = get_locator_heights(tip.height);

        // Clear history_locator in any case of header chain rollback.
        if self.history_locator.len() > 0
            && tip.last_block_hash != chain.header_head()?.last_block_hash
        {
            self.history_locator.retain(|&x| x.0 == 0);
        }

        // For each height we need, we either check if something is close enough from last locator, or go to the db.
        let mut locator: Vec<(u64, CryptoHash)> = vec![(tip.height, tip.last_block_hash)];
        for h in heights {
            if let Some(x) = close_enough(&self.history_locator, h) {
                locator.push(x);
            } else {
                // Walk backwards to find last known hash.
                let last_loc = locator.last().unwrap().clone();
                if let Ok(header) = chain.get_header_by_height(h) {
                    if header.inner.height != last_loc.0 {
                        locator.push((header.inner.height, header.hash()));
                    }
                }
            }
        }
        locator.dedup_by(|a, b| a.0 == b.0);
        debug!(target: "sync", "Sync: locator: {:?}", locator);
        self.history_locator = locator.clone();
        Ok(locator.iter().map(|x| x.1).collect())
    }
}

/// Check if there is a close enough value to provided height in the locator.
fn close_enough(locator: &Vec<(u64, CryptoHash)>, height: u64) -> Option<(u64, CryptoHash)> {
    if locator.len() == 0 {
        return None;
    }
    // Check boundaries, if lower than the last.
    if locator.last().unwrap().0 >= height {
        return locator.last().map(|x| x.clone());
    }
    // Higher than first and first is within acceptable gap.
    if locator[0].0 < height && height.saturating_sub(127) < locator[0].0 {
        return Some(locator[0]);
    }
    for h in locator.windows(2) {
        if height <= h[0].0 && height > h[1].0 {
            if h[0].0 - height < height - h[1].0 {
                return Some(h[0].clone());
            } else {
                return Some(h[1].clone());
            }
        }
    }
    None
}

/// Given height stepping back to 0 in powers of 2 steps.
fn get_locator_heights(height: u64) -> Vec<u64> {
    let mut current = height;
    let mut heights = vec![];
    while current > 0 {
        heights.push(current);
        if heights.len() >= MAX_BLOCK_HEADER_HASHES as usize - 1 {
            break;
        }
        let next = 2u64.pow(heights.len() as u32);
        current = if current > next { current - next } else { 0 };
    }
    heights.push(0);
    heights
}

/// Helper to track block syncing.
pub struct BlockSync {
    network_adapter: Box<dyn SyncNetworkAdapter>,
    blocks_requested: BlockIndex,
    receive_timeout: DateTime<Utc>,
    prev_blocks_received: BlockIndex,
    /// How far to fetch blocks vs fetch state.
    block_fetch_horizon: BlockIndex,
}

impl BlockSync {
    pub fn new(
        network_adapter: Box<dyn SyncNetworkAdapter>,
        block_fetch_horizon: BlockIndex,
    ) -> Self {
        BlockSync {
            network_adapter,
            blocks_requested: 0,
            receive_timeout: Utc::now(),
            prev_blocks_received: 0,
            block_fetch_horizon,
        }
    }

    /// Runs check if block sync is needed, if it's needed and it's too far - sync state is started instead (returning true).
    /// Otherwise requests recent blocks from peers.
    pub fn run(
        &mut self,
        sync_status: &mut SyncStatus,
        chain: &mut Chain,
        highest_height: BlockIndex,
        most_weight_peers: &[FullPeerInfo],
    ) -> Result<bool, near_chain::Error> {
        if self.block_sync_due(chain)? {
            if self.block_sync(chain, most_weight_peers, self.block_fetch_horizon)? {
                return Ok(true);
            }

            let head = chain.head()?;
            *sync_status = SyncStatus::BodySync { current_height: head.height, highest_height };
        }
        Ok(false)
    }

    /// Returns true if state download is required (last known block is too far).
    /// Otherwise request recent blocks from peers round robin.
    pub fn block_sync(
        &mut self,
        chain: &mut Chain,
        most_weight_peers: &[FullPeerInfo],
        block_fetch_horizon: BlockIndex,
    ) -> Result<bool, near_chain::Error> {
        let (state_needed, mut hashes) = chain.check_state_needed(block_fetch_horizon)?;
        if state_needed {
            return Ok(true);
        }
        hashes.reverse();
        // Ask for `num_peers * MAX_PEER_BLOCK_REQUEST` blocks up to 100, throttle if there is too many orphans in the chain.
        let block_count = cmp::min(
            cmp::min(MAX_BLOCK_REQUEST, MAX_PEER_BLOCK_REQUEST * most_weight_peers.len()),
            near_chain::MAX_ORPHAN_SIZE.saturating_sub(chain.orphans_len()) + 1,
        );

        let hashes_to_request = hashes
            .iter()
            .filter(|x| !chain.get_block(x).is_ok() && !chain.is_orphan(x))
            .take(block_count)
            .collect::<Vec<_>>();
        if hashes_to_request.len() > 0 {
            let head = chain.head()?;
            let header_head = chain.header_head()?;

            debug!(target: "sync", "Block sync: {}/{} requesting blocks {:?} from {} peers", head.height, header_head.height, hashes_to_request, most_weight_peers.len());

            self.blocks_requested = 0;
            self.receive_timeout = Utc::now() + Duration::seconds(BLOCK_REQUEST_TIMEOUT);

            let mut peers_iter = most_weight_peers.iter().cycle();
            for hash in hashes_to_request.into_iter() {
                if let Some(peer) = peers_iter.next() {
                    self.network_adapter.send(NetworkRequests::BlockRequest {
                        hash: hash.clone(),
                        peer_id: peer.peer_info.id.clone(),
                    });
                    self.blocks_requested += 1;
                }
            }
        }
        Ok(false)
    }

    /// Check if we should run block body sync and ask for more full blocks.
    fn block_sync_due(&mut self, chain: &Chain) -> Result<bool, near_chain::Error> {
        let blocks_received = self.blocks_received(chain)?;

        // Some blocks have been requested.
        if self.blocks_requested > 0 {
            let timeout = Utc::now() > self.receive_timeout;
            if timeout && blocks_received <= self.prev_blocks_received {
                debug!(target: "sync", "Block sync: expecting {} more blocks and none received for a while", self.blocks_requested);
                return Ok(true);
            }
        }

        if blocks_received > self.prev_blocks_received {
            // Some blocks received, update for next check.
            self.receive_timeout = Utc::now() + Duration::seconds(BLOCK_SOME_RECEIVED_TIMEOUT);
            self.blocks_requested =
                self.blocks_requested.saturating_sub(blocks_received - self.prev_blocks_received);
            self.prev_blocks_received = blocks_received;
        }

        // Account for broadcast adding few blocks to orphans during.
        if self.blocks_requested < BLOCK_REQUEST_BROADCAST_OFFSET {
            debug!(target: "sync", "Block sync: No pending block requests, requesting more.");
            return Ok(true);
        }

        Ok(false)
    }

    /// Total number of received blocks by the chain.
    fn blocks_received(&self, chain: &Chain) -> Result<u64, near_chain::Error> {
        Ok((chain.head()?).height + chain.orphans_len() as u64 + chain.orphans_evicted_len() as u64)
    }
}

/// Helper to track state sync.
pub struct StateSync {
    network_adapter: Box<dyn SyncNetworkAdapter>,
    state_fetch_horizon: BlockIndex,

    syncing_peers: HashMap<ShardId, FullPeerInfo>,
    prev_state_sync: HashMap<ShardId, DateTime<Utc>>,
}

impl StateSync {
    pub fn new(
        network_adapter: Box<dyn SyncNetworkAdapter>,
        state_fetch_horizon: BlockIndex,
    ) -> Self {
        StateSync {
            network_adapter,
            state_fetch_horizon,
            syncing_peers: Default::default(),
            prev_state_sync: Default::default(),
        }
    }

    fn find_sync_hash(&self, chain: &mut Chain) -> Result<CryptoHash, near_chain::Error> {
        let header_head = chain.header_head()?;
        let mut sync_hash = header_head.prev_block_hash;
        for _ in 0..self.state_fetch_horizon {
            sync_hash = chain.get_block_header(&sync_hash)?.inner.prev_hash;
        }
        Ok(sync_hash)
    }

    pub fn run(
        &mut self,
        sync_status: &mut SyncStatus,
        chain: &mut Chain,
        highest_height: BlockIndex,
        most_weight_peers: &Vec<FullPeerInfo>,
        tracking_shards: Vec<ShardId>,
    ) -> Result<(), near_chain::Error> {
        let header_head = chain.header_head()?;
        let mut sync_need_restart = HashSet::new();

        let (sync_hash, mut new_shard_sync) = match &sync_status {
            SyncStatus::StateSync(sync_hash, shard_sync) => (sync_hash.clone(), shard_sync.clone()),
            _ => (self.find_sync_hash(chain)?, HashMap::default()),
        };

        // Check syncing peer connection status.
        let mut all_done = false;
        if let SyncStatus::StateSync(_, shard_statuses) = sync_status {
            all_done = true;
            for (shard_id, shard_status) in shard_statuses.iter() {
                all_done = all_done && ShardSyncStatus::StateDone == *shard_status;
                if let ShardSyncStatus::Error(error) = shard_status {
                    error!(target: "sync", "State sync: shard {} sync failed: {}", shard_id, error);
                    sync_need_restart.insert(shard_id);
                } else if let Some(ref peer) = self.syncing_peers.get(shard_id) {
                    if let ShardSyncStatus::StateDownload { .. } = shard_status {
                        if !most_weight_peers.contains(peer) {
                            sync_need_restart.insert(shard_id);
                            info!(target: "sync", "State sync: peer connection lost: {:?}, restart shard {}", peer.peer_info.id, shard_id);
                        }
                    }
                }
            }
        }

        if all_done {
            info!(target: "sync", "State sync: all shards are done");

            // TODO(1046): this code belongs in chain, but waiting to see where chunks will fit.

            // Get header we were syncing into.
            let header = chain.get_block_header(&sync_hash)?;
            let hash = header.inner.prev_hash;
            let prev_header = chain.get_block_header(&hash)?;
            let tip = Tip::from_header(prev_header);
            // Update related heads now.
            let mut chain_store_update = chain.mut_store().store_update();
            chain_store_update.save_body_head(&tip);
            chain_store_update.save_body_tail(&tip);
            chain_store_update.commit()?;

            // Check if thare are any orphans unlocked by this state sync.
            chain.check_orphans(hash, |_, _, _| {});

            *sync_status = SyncStatus::BodySync { current_height: 0, highest_height: 0 };
            self.prev_state_sync.clear();
            self.syncing_peers.clear();
            return Ok(());
        }

        let now = Utc::now();
        let mut update_sync_status = false;
        for shard_id in tracking_shards {
            if sync_need_restart.contains(&shard_id) || header_head.height == highest_height {
                let (go, download_timeout) = match self.prev_state_sync.get(&shard_id) {
                    None => {
                        self.prev_state_sync.insert(shard_id, now);
                        (true, false)
                    }
                    Some(prev) => (false, now - *prev > Duration::minutes(STATE_SYNC_TIMEOUT)),
                };

                if download_timeout {
                    error!(target: "sync", "State sync: state download for shard {} timed out in {} minutes", shard_id, STATE_SYNC_TIMEOUT);
                }

                if go || download_timeout {
                    match self.request_state(shard_id, chain, sync_hash, most_weight_peers) {
                        Some(peer) => {
                            self.syncing_peers.insert(shard_id, peer);
                            new_shard_sync.insert(
                                shard_id,
                                ShardSyncStatus::StateDownload {
                                    start_time: now,
                                    prev_update_time: now,
                                    prev_downloaded_size: 0,
                                    downloaded_size: 0,
                                    total_size: 0,
                                },
                            );
                        }
                        None => {
                            new_shard_sync.insert(
                                shard_id,
                                ShardSyncStatus::Error(format!(
                                    "Failed to find peer with state for shard {}",
                                    shard_id
                                )),
                            );
                        }
                    }
                    update_sync_status = true;
                }
            }
        }
        if update_sync_status {
            *sync_status = SyncStatus::StateSync(sync_hash, new_shard_sync);
        }
        Ok(())
    }

    fn request_state(
        &mut self,
        shard_id: ShardId,
        _chain: &Chain,
        hash: CryptoHash,
        most_weight_peers: &Vec<FullPeerInfo>,
    ) -> Option<FullPeerInfo> {
        if let Some(peer) = most_weight_peer(most_weight_peers) {
            self.network_adapter.send(NetworkRequests::StateRequest {
                shard_id,
                hash,
                peer_id: peer.peer_info.id,
            });
            return Some(peer);
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use near_chain::test_utils::setup;
    use near_chain::Provenance;
    use near_network::types::PeerChainInfo;
    use near_network::PeerInfo;
    use near_primitives::block::Block;
    use std::sync::{Arc, RwLock};

    #[derive(Default)]
    struct MockNetworkAdapter {
        pub requests: Arc<RwLock<Vec<NetworkRequests>>>,
    }

    impl SyncNetworkAdapter for MockNetworkAdapter {
        fn send(&self, msg: NetworkRequests) {
            self.requests.write().unwrap().push(msg);
        }
    }

    #[test]
    fn test_get_locator_heights() {
        assert_eq!(get_locator_heights(0), vec![0]);
        assert_eq!(get_locator_heights(1), vec![1, 0]);
        assert_eq!(get_locator_heights(2), vec![2, 0]);
        assert_eq!(get_locator_heights(3), vec![3, 1, 0]);
        assert_eq!(get_locator_heights(10), vec![10, 8, 4, 0]);
        assert_eq!(get_locator_heights(100), vec![100, 98, 94, 86, 70, 38, 0]);
        assert_eq!(
            get_locator_heights(1000),
            vec![1000, 998, 994, 986, 970, 938, 874, 746, 490, 0]
        );
        // Locator is still reasonable size even given large height.
        assert_eq!(
            get_locator_heights(10000),
            vec![10000, 9998, 9994, 9986, 9970, 9938, 9874, 9746, 9490, 8978, 7954, 5906, 1810, 0,]
        );
    }

    /// Starts two chains that fork of genesis and checks that they can sync heaaders to the longest.
    #[test]
    fn test_sync_headers_fork() {
        let requests = Arc::new(RwLock::new(vec![]));
        let mock_adapter = Box::new(MockNetworkAdapter { requests: requests.clone() });
        let mut header_sync = HeaderSync::new(mock_adapter);
        let (mut chain, _, signer) = setup();
        for _ in 0..5 {
            let prev = chain.head_header().unwrap();
            let block = Block::empty(&prev, signer.clone());
            chain.process_block(block, Provenance::PRODUCED, |_, _, _| {}).unwrap();
        }
        let (mut chain2, _, signer2) = setup();
        for _ in 0..10 {
            let prev = chain2.head_header().unwrap();
            let block = Block::empty(&prev, signer2.clone());
            chain2.process_block(block, Provenance::PRODUCED, |_, _, _| {}).unwrap();
        }
        let mut sync_status = SyncStatus::NoSync;
        let peer1 = FullPeerInfo {
            peer_info: PeerInfo::random(),
            chain_info: PeerChainInfo {
                genesis: chain.genesis().hash(),
                height: chain2.head().unwrap().height,
                total_weight: chain2.head().unwrap().total_weight,
            },
        };
        let head = chain.head().unwrap();
        assert!(header_sync
            .run(&mut sync_status, &mut chain, head.height, &vec![peer1.clone()])
            .is_ok());
        assert!(sync_status.is_syncing());
        // Check that it queried last block, and then stepped down to genesis block to find common block with the peer.
        assert_eq!(
            requests.read().unwrap()[0],
            NetworkRequests::BlockHeadersRequest {
                hashes: [5, 3, 0]
                    .iter()
                    .map(|i| chain.get_block_by_height(*i).unwrap().hash())
                    .collect(),
                peer_id: peer1.peer_info.id
            }
        );
    }
}
