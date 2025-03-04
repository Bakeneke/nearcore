//! Readonly view of the chain and state of the database.
//! Useful for querying from RPC.

use std::sync::Arc;

use actix::{Actor, Context, Handler};
use chrono::{DateTime, Utc};

use near_chain::{Chain, ErrorKind, RuntimeAdapter};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{TransactionResult, TransactionStatus};
use near_primitives::views::{
    BlockView, FinalTransactionResult, FinalTransactionStatus, QueryResponse, TransactionLogView,
    TransactionResultView,
};
use near_store::Store;

use crate::types::{Error, GetBlock, Query, TxStatus};
use crate::TxDetails;
use near_primitives::types::BlockIndex;

/// View client provides currently committed (to the storage) view of the current chain and state.
pub struct ViewClientActor {
    chain: Chain,
    runtime_adapter: Arc<dyn RuntimeAdapter>,
}

impl ViewClientActor {
    pub fn new(
        store: Arc<Store>,
        genesis_time: DateTime<Utc>,
        runtime_adapter: Arc<dyn RuntimeAdapter>,
        transaction_validity_period: BlockIndex,
    ) -> Result<Self, Error> {
        // TODO: should we create shared ChainStore that is passed to both Client and ViewClient?
        let chain =
            Chain::new(store, runtime_adapter.clone(), genesis_time, transaction_validity_period)?;
        Ok(ViewClientActor { chain, runtime_adapter })
    }

    pub fn get_transaction_result(
        &mut self,
        hash: &CryptoHash,
    ) -> Result<TransactionResultView, String> {
        match self.chain.get_transaction_result(hash) {
            Ok(result) => Ok(result.clone().into()),
            Err(err) => match err.kind() {
                ErrorKind::DBNotFoundErr(_) => Ok(TransactionResult {
                    status: TransactionStatus::Unknown,
                    ..Default::default()
                }
                .into()),
                _ => Err(err.to_string()),
            },
        }
    }

    fn get_recursive_transaction_results(
        &mut self,
        hash: &CryptoHash,
    ) -> Result<Vec<TransactionLogView>, String> {
        let result = self.get_transaction_result(hash)?;
        let receipt_ids = result.receipts.clone();
        let mut transactions = vec![TransactionLogView { hash: hash.clone().into(), result }];
        for hash in &receipt_ids {
            transactions
                .extend(self.get_recursive_transaction_results(&hash.clone().into())?.into_iter());
        }
        Ok(transactions)
    }

    fn get_final_transaction_result(
        &mut self,
        hash: &CryptoHash,
    ) -> Result<FinalTransactionResult, String> {
        let transactions = self.get_recursive_transaction_results(hash)?;
        let status = if transactions
            .iter()
            .find(|t| &t.result.status == &TransactionStatus::Failed)
            .is_some()
        {
            FinalTransactionStatus::Failed
        } else if transactions
            .iter()
            .find(|t| &t.result.status == &TransactionStatus::Unknown)
            .is_some()
        {
            FinalTransactionStatus::Started
        } else {
            FinalTransactionStatus::Completed
        };
        Ok(FinalTransactionResult {
            status,
            transactions: transactions.into_iter().map(|t| t.into()).collect(),
        })
    }
}

impl Actor for ViewClientActor {
    type Context = Context<Self>;
}

/// Handles runtime query.
impl Handler<Query> for ViewClientActor {
    type Result = Result<QueryResponse, String>;

    fn handle(&mut self, msg: Query, _: &mut Context<Self>) -> Self::Result {
        let head = self.chain.head().map_err(|err| err.to_string())?;
        let state_root =
            self.chain.get_post_state_root(&head.last_block_hash).map_err(|err| err.to_string())?;
        self.runtime_adapter
            .query(*state_root, head.height, &msg.path, &msg.data)
            .map_err(|err| err.to_string())
    }
}

/// Handles retrieving block from the chain.
impl Handler<GetBlock> for ViewClientActor {
    type Result = Result<BlockView, String>;

    fn handle(&mut self, msg: GetBlock, _: &mut Context<Self>) -> Self::Result {
        match msg {
            GetBlock::Best => match self.chain.head() {
                Ok(head) => self.chain.get_block(&head.last_block_hash).map(Clone::clone),
                Err(err) => Err(err),
            },
            GetBlock::Height(height) => self.chain.get_block_by_height(height).map(Clone::clone),
            GetBlock::Hash(hash) => self.chain.get_block(&hash).map(Clone::clone),
        }
        .map(|block| block.into())
        .map_err(|err| err.to_string())
    }
}

impl Handler<TxStatus> for ViewClientActor {
    type Result = Result<FinalTransactionResult, String>;

    fn handle(&mut self, msg: TxStatus, _: &mut Context<Self>) -> Self::Result {
        self.get_final_transaction_result(&msg.tx_hash)
    }
}

impl Handler<TxDetails> for ViewClientActor {
    type Result = Result<TransactionResultView, String>;

    fn handle(&mut self, msg: TxDetails, _: &mut Context<Self>) -> Self::Result {
        self.get_transaction_result(&msg.tx_hash)
    }
}
