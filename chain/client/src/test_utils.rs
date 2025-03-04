use std::sync::{Arc, RwLock};

use actix::actors::mocker::Mocker;
use actix::{Actor, Addr, AsyncContext, Context, Recipient};
use chrono::Utc;

use near_chain::test_utils::KeyValueRuntime;
use near_crypto::{InMemorySigner, KeyType, PublicKey};
use near_network::types::NetworkInfo;
use near_network::{NetworkRequests, NetworkResponses, PeerManagerActor};
use near_store::test_utils::create_test_store;
use near_telemetry::TelemetryActor;

use crate::{BlockProducer, ClientActor, ClientConfig, ViewClientActor};
use near_primitives::types::BlockIndex;

pub type NetworkMock = Mocker<PeerManagerActor>;

/// Sets up ClientActor and ViewClientActor viewing the same store/runtime.
pub fn setup(
    validators: Vec<&str>,
    account_id: &str,
    skip_sync_wait: bool,
    recipient: Recipient<NetworkRequests>,
    tx_validity_period: BlockIndex,
) -> (ClientActor, ViewClientActor) {
    let store = create_test_store();
    let runtime = Arc::new(KeyValueRuntime::new_with_validators(
        store.clone(),
        validators.into_iter().map(Into::into).collect(),
    ));
    let signer = Arc::new(InMemorySigner::from_seed(account_id, KeyType::ED25519, account_id));
    let genesis_time = Utc::now();
    let telemetry = TelemetryActor::default().start();
    let view_client = ViewClientActor::new(
        store.clone(),
        genesis_time.clone(),
        runtime.clone(),
        tx_validity_period,
    )
    .unwrap();
    let mut config = ClientConfig::test(skip_sync_wait);
    config.transaction_validity_period = tx_validity_period;
    let client = ClientActor::new(
        config,
        store,
        genesis_time,
        runtime,
        PublicKey::empty(KeyType::ED25519).into(),
        recipient,
        Some(signer.into()),
        telemetry,
    )
    .unwrap();
    (client, view_client)
}

/// Sets up ClientActor and ViewClientActor with mock PeerManager.
pub fn setup_mock(
    validators: Vec<&'static str>,
    account_id: &'static str,
    skip_sync_wait: bool,
    network_mock: Box<
        dyn FnMut(
            &NetworkRequests,
            &mut Context<NetworkMock>,
            Addr<ClientActor>,
        ) -> NetworkResponses,
    >,
) -> (Addr<ClientActor>, Addr<ViewClientActor>) {
    setup_mock_with_validity_period(validators, account_id, skip_sync_wait, network_mock, 100)
}

pub fn setup_mock_with_validity_period(
    validators: Vec<&'static str>,
    account_id: &'static str,
    skip_sync_wait: bool,
    mut network_mock: Box<
        dyn FnMut(
            &NetworkRequests,
            &mut Context<NetworkMock>,
            Addr<ClientActor>,
        ) -> NetworkResponses,
    >,
    validity_period: BlockIndex,
) -> (Addr<ClientActor>, Addr<ViewClientActor>) {
    let view_client_addr = Arc::new(RwLock::new(None));
    let view_client_addr1 = view_client_addr.clone();
    let client_addr = ClientActor::create(move |ctx| {
        let client_addr = ctx.address();
        let pm = NetworkMock::mock(Box::new(move |msg, ctx| {
            let msg = msg.downcast_ref::<NetworkRequests>().unwrap();
            let resp = network_mock(msg, ctx, client_addr.clone());
            Box::new(Some(resp))
        }))
        .start();
        let (client, view_client) =
            setup(validators, account_id, skip_sync_wait, pm.recipient(), validity_period);
        *view_client_addr1.write().unwrap() = Some(view_client.start());
        client
    });
    (client_addr, view_client_addr.clone().read().unwrap().clone().unwrap())
}

/// Sets up ClientActor and ViewClientActor without network.
pub fn setup_no_network(
    validators: Vec<&'static str>,
    account_id: &'static str,
    skip_sync_wait: bool,
) -> (Addr<ClientActor>, Addr<ViewClientActor>) {
    setup_no_network_with_validity_period(validators, account_id, skip_sync_wait, 100)
}

pub fn setup_no_network_with_validity_period(
    validators: Vec<&'static str>,
    account_id: &'static str,
    skip_sync_wait: bool,
    validity_period: BlockIndex,
) -> (Addr<ClientActor>, Addr<ViewClientActor>) {
    setup_mock_with_validity_period(
        validators,
        account_id,
        skip_sync_wait,
        Box::new(|req, _, _| match req {
            NetworkRequests::FetchInfo { .. } => NetworkResponses::Info(NetworkInfo {
                num_active_peers: 0,
                peer_max_count: 0,
                most_weight_peers: vec![],
                received_bytes_per_sec: 0,
                sent_bytes_per_sec: 0,
                routes: None,
            }),
            _ => NetworkResponses::NoResponse,
        }),
        validity_period,
    )
}

impl BlockProducer {
    pub fn test(seed: &str) -> Self {
        Arc::new(InMemorySigner::from_seed(seed, KeyType::ED25519, seed)).into()
    }
}
